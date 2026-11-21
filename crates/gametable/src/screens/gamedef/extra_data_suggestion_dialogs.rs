//! Card View's "Suggest Extra Data" dialogs, for several selected cards: choose the extra field (or
//! a new one), how to read it (local text recognition or Claude) and where it is printed, watch the
//! progress, then review the values before any card changes.

use crate::app::Frame;
use crate::data::image_resolver::resolve_bare_image_path;
use crate::data::settings::keys;
use crate::data::title_suggestion::claude_vision::ClaudeModel;
use crate::data::title_suggestion::extra_data::{current_field_value, pick_example_values, set_extra_field};
use crate::data::title_suggestion::local_ocr;
use crate::data::title_suggestion::{
    CardToSuggest, ClaudeAccess, ImageEngine, SuggestionContext, SuggestionJob, SuggestionTarget, TitleSource,
};
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::display_card_backs;
use crate::screens::gamedef::suggestion_review::{ReviewCards, ReviewRow, draw_review_rows};
use crate::screens::gamedef::title_area_picker::{AreaLabels, TitleAreaPicker};
use crate::screens::gamedef::title_suggestion_dialogs::claude_api_key;
use crate::ui::card_filter_bar::extra_field_names;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::Weight;
use crate::ui::widgets::Choice;

/// Which field the run fills in.
#[derive(Debug, Clone, PartialEq)]
enum FieldChoice {
    /// A field some card already has.
    Existing(String),
    /// A new field, named in the "Field key" box.
    CreateNew,
}

/// What reads the cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReaderChoice {
    LocalOcr,
    Claude,
}

impl ReaderChoice {
    fn image_engine(self) -> ImageEngine {
        match self {
            ReaderChoice::LocalOcr => ImageEngine::LocalOcr,
            ReaderChoice::Claude => ImageEngine::Claude,
        }
    }

    fn setting_value(self) -> &'static str {
        match self {
            ReaderChoice::LocalOcr => "ocr",
            ReaderChoice::Claude => "claude",
        }
    }

    fn from_setting_value(value: Option<&str>) -> ReaderChoice {
        if value == Some("claude") { ReaderChoice::Claude } else { ReaderChoice::LocalOcr }
    }
}

enum Stage {
    Choosing,
    Running { job: SuggestionJob },
    Reviewing { rows: Vec<ReviewRow>, summary: String },
}

/// The open "Suggest Extra Data" flow.
pub struct SuggestExtraDataFlow {
    stage: Stage,
    /// The pool's selection when opened, in card order.
    selected_cards: Vec<u64>,
    /// Every extra field name the game's cards use.
    existing_fields: Vec<String>,
    field_choice: FieldChoice,
    /// The typed key when creating a new field.
    new_field_key: String,
    reader: ReaderChoice,
    /// Where the value is printed, drawn over the first selected card.
    field_area: TitleAreaPicker,
    /// The user's own instructions for Claude about this field (optional).
    custom_instructions: String,
    /// The field whose remembered instructions were last loaded into `custom_instructions`.
    instructions_loaded_for: Option<String>,
    /// The field being filled in, fixed when the run starts.
    running_field_key: String,
}

/// Where a field's Claude instructions are remembered: per game and field (ignoring case).
fn instructions_setting_key(game_id: &str, field_key: &str) -> String {
    format!("{}_{game_id}_{}", keys::SUGGEST_EXTRA_DATA_INSTRUCTIONS_PREFIX, field_key.to_lowercase())
}

impl SuggestExtraDataFlow {
    pub fn open(frame: &Frame, model: &EditorModel, selected_cards: Vec<u64>) -> SuggestExtraDataFlow {
        let mut reader = ReaderChoice::from_setting_value(frame.settings.text(keys::SUGGEST_EXTRA_DATA_SOURCE));
        if reader == ReaderChoice::Claude && claude_api_key(frame).is_none() {
            reader = ReaderChoice::LocalOcr;
        }
        let existing_fields = extra_field_names(&model.game);
        let field_choice =
            existing_fields.first().map_or(FieldChoice::CreateNew, |name| FieldChoice::Existing(name.clone()));
        let mut selected_cards = selected_cards;
        selected_cards.sort_by_key(|&key| model.card_keys.index_of(key));
        SuggestExtraDataFlow {
            stage: Stage::Choosing,
            selected_cards,
            existing_fields,
            field_choice,
            new_field_key: String::new(),
            reader,
            field_area: TitleAreaPicker::default(),
            custom_instructions: String::new(),
            instructions_loaded_for: None,
            running_field_key: String::new(),
        }
    }

    /// Draws the current step; returns false once the flow has closed.
    pub fn show(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        match self.stage {
            Stage::Choosing => self.show_chooser(frame, model),
            Stage::Running { .. } => self.show_progress(frame, model),
            Stage::Reviewing { .. } => self.show_review(frame, model),
        }
    }

    /// The field key the dialog's choices name; a new key matching an existing field (ignoring
    /// case) takes that field's spelling.
    fn chosen_field_key(&self) -> Option<String> {
        match &self.field_choice {
            FieldChoice::Existing(name) => Some(name.clone()),
            FieldChoice::CreateNew => {
                let typed = self.new_field_key.trim();
                if typed.is_empty() {
                    return None;
                }
                let existing = self.existing_fields.iter().find(|name| name.eq_ignore_ascii_case(typed));
                Some(existing.cloned().unwrap_or_else(|| typed.to_string()))
            }
        }
    }

    /// When the chosen field changes, loads the instructions remembered for it (if any; a field
    /// with none keeps what is typed, so naming a new field doesn't clear the box).
    fn load_remembered_instructions(&mut self, frame: &Frame, model: &EditorModel) {
        let field_key = self.chosen_field_key();
        if field_key == self.instructions_loaded_for {
            return;
        }
        if let Some(remembered) =
            field_key.as_deref().and_then(|key| frame.settings.text(&instructions_setting_key(&model.game.id, key)))
        {
            self.custom_instructions = remembered.to_string();
        }
        self.instructions_loaded_for = field_key;
    }

    fn show_chooser(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        self.load_remembered_instructions(frame, model);
        let has_key = claude_api_key(frame).is_some();
        let models_missing = local_ocr::models_missing(&local_ocr::models_folder(&frame.paths.app_folder));
        let card_count = self.selected_cards.iter().filter(|&&key| model.card_keys.index_of(key).is_some()).count();
        let needs_area = self.reader == ReaderChoice::LocalOcr && self.field_area.area.is_none();
        let can_start = card_count > 0 && self.chosen_field_key().is_some() && !needs_area;
        let start_label = format!("Suggest {card_count} Values");
        let actions = [DialogAction::text("Cancel"), DialogAction::filled(&start_label).disabled(!can_start)];
        let spec =
            DialogSpec { title: Some("Suggest Extra Data"), width: 520.0, actions: &actions, ..DialogSpec::default() };
        let palette = frame.palette();
        let claude_model = ClaudeModel::from_setting_value(frame.settings.text(keys::CLAUDE_TITLE_MODEL));
        let mut field_choices: Vec<Choice<FieldChoice>> = self
            .existing_fields
            .iter()
            .map(|name| Choice::new(FieldChoice::Existing(name.clone()), name.clone()))
            .collect();
        field_choices.push(Choice::new(FieldChoice::CreateNew, "Create new extra data field..."));
        let first_card = self.selected_cards.first().and_then(|&key| model.card_keys.index_of(key));
        let field_name_for_hint = self.chosen_field_key();
        let instructions_height = frame.text_area_height(3, None);
        let (field_choice, new_field_key, reader, field_area, custom_instructions) = (
            &mut self.field_choice,
            &mut self.new_field_key,
            &mut self.reader,
            &mut self.field_area,
            &mut self.custom_instructions,
        );
        let response = frame.dialog("suggestextra", &spec, |frame, x, y, width| {
            let mut row_y = y;
            let label = TextOptions::new().medium().width(width);
            let note_options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);

            frame.text("Extra data field", x, row_y, label);
            row_y += 24.0 + 4.0;
            let dropdown_options = DropdownOptions {
                label: Some("Field"),
                height: Some(44.0),
                label_background: Some(palette.surface_container_high),
                ..DropdownOptions::default()
            };
            *field_choice = frame.dropdown(
                "suggestextra:field",
                x,
                row_y,
                width,
                &field_choices,
                field_choice.clone(),
                &dropdown_options,
            );
            row_y += 44.0 + 12.0;
            if *field_choice == FieldChoice::CreateNew {
                let key_options = TextFieldOptions {
                    height: Some(44.0),
                    label_background: Some(palette.surface_container_high),
                    ..TextFieldOptions::label("Field key")
                };
                frame.text_field("suggestextra:newkey", x, row_y, width, new_field_key, &key_options);
                row_y += 44.0 + 12.0;
            }

            frame.text("Read the value from", x, row_y, label);
            row_y += 24.0;
            let reader_choices = [
                Choice::new(ReaderChoice::LocalOcr, "Image (local)"),
                Choice::new(ReaderChoice::Claude, "Image (Claude)"),
            ];
            let picked = frame.segmented("suggestextra:reader", x, row_y, width, &reader_choices, *reader, None, false);
            *reader = if picked == ReaderChoice::Claude && !has_key { *reader } else { picked };
            row_y += 40.0 + 8.0;
            let mut note = match *reader {
                ReaderChoice::LocalOcr => {
                    let mut note = "Reads the text inside the box you draw below with text recognition on this \
                                    computer. Free and offline."
                        .to_string();
                    if models_missing {
                        note += " The first run downloads its models (about 20 MB).";
                    }
                    note
                }
                ReaderChoice::Claude => format!(
                    "Sends each card's image to Claude ({}) through the Anthropic API, along with the field's name \
                     and other cards' values for it. Billed to your API key's account.",
                    match claude_model {
                        ClaudeModel::Haiku => "Haiku 5.5",
                        ClaudeModel::Opus => "Opus 5.5",
                    }
                ),
            };
            if !has_key {
                note += "\nTo use Claude, add your Claude API key in Settings.";
            }
            frame.text(&note, x, row_y, note_options);
            row_y += frame.text_height(&note, width, 12.0, Weight::Regular) + 8.0;
            if *reader == ReaderChoice::Claude {
                row_y += 4.0;
                let instructions_options = TextFieldOptions {
                    label_background: Some(palette.surface_container_high),
                    hint: Some("e.g. Write the ink colour in lower case; use \"none\" if the card has no cost."),
                    ..TextFieldOptions::label("Instructions for Claude (optional)")
                };
                frame.text_area(
                    "suggestextra:instructions",
                    x,
                    row_y,
                    width,
                    instructions_height,
                    custom_instructions,
                    &instructions_options,
                );
                row_y += instructions_height + 4.0;
                let remember_note = "Remembered for this field in this game.";
                frame.text(remember_note, x, row_y, note_options);
                row_y += 18.0 + 8.0;
            }
            let count_text = format!("{card_count} cards. You'll review the values before anything changes.");
            frame.text(&count_text, x, row_y, note_options);
            row_y += 18.0 + 16.0;

            if let Some(index) = first_card {
                let card = &model.game.cards()[index];
                let image_path = card
                    .image_path
                    .as_deref()
                    .map(|image| resolve_bare_image_path(&model.folder_path, image, card.set_id.as_deref()));
                let card_name =
                    if card.card_title.trim().is_empty() { card.id.as_str() } else { card.card_title.as_str() };
                let subject = match &field_name_for_hint {
                    Some(name) => format!("the \"{name}\" value"),
                    None => "the value".to_string(),
                };
                let labels = AreaLabels {
                    heading: "Field area",
                    subject: &subject,
                    required: *reader == ReaderChoice::LocalOcr,
                };
                row_y += field_area.draw(
                    frame,
                    "suggestextra:area",
                    labels,
                    image_path.as_deref(),
                    card_name,
                    x,
                    row_y,
                    width,
                );
            }
            row_y - y
        });
        match response {
            DialogResponse::Action(1) if can_start => {
                frame.settings.set(keys::SUGGEST_EXTRA_DATA_SOURCE, self.reader.setting_value());
                if self.reader == ReaderChoice::Claude
                    && let Some(field_key) = self.chosen_field_key()
                {
                    let setting_key = instructions_setting_key(&model.game.id, &field_key);
                    frame.settings.set(&setting_key, self.custom_instructions.trim());
                }
                self.start(frame, model);
                true
            }
            DialogResponse::Action(1) | DialogResponse::Open | DialogResponse::Enter => true,
            _ => false,
        }
    }

    fn start(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        let Some(field_key) = self.chosen_field_key() else { return };
        let cards: Vec<CardToSuggest> = self
            .selected_cards
            .iter()
            .filter_map(|&key| model.card_keys.index_of(key).map(|index| (key, index)))
            .map(|(key, index)| {
                let card = model.game.cards()[index].clone();
                let image_path = card
                    .image_path
                    .as_deref()
                    .map(|image| resolve_bare_image_path(&model.folder_path, image, card.set_id.as_deref()));
                CardToSuggest { key, card, image_path }
            })
            .collect();
        let context = SuggestionContext {
            game_name: model.game.name.clone(),
            target: SuggestionTarget::ExtraField {
                key: field_key.clone(),
                example_values: pick_example_values(&model.game, &field_key),
                custom_instructions: if self.reader == ReaderChoice::Claude {
                    self.custom_instructions.trim().to_string()
                } else {
                    String::new()
                },
            },
            title_area: self.field_area.area,
            title_style: Default::default(),
            claude: claude_api_key(frame).map(|api_key| ClaudeAccess {
                api_key,
                model: ClaudeModel::from_setting_value(frame.settings.text(keys::CLAUDE_TITLE_MODEL)),
            }),
            ocr_models_folder: local_ocr::models_folder(&frame.paths.app_folder),
        };
        self.running_field_key = field_key;
        let job = SuggestionJob::start(cards, TitleSource::FromImage(self.reader.image_engine()), context);
        self.stage = Stage::Running { job };
    }

    fn show_progress(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        let Stage::Running { job } = &mut self.stage else { return true };
        let finished = job.poll();
        let (done, total) = (job.outcomes.len(), job.total());
        let cancelled = job.is_cancelled();
        if finished || (cancelled && done > 0) {
            let outcomes = std::mem::take(&mut job.outcomes);
            let field_key = &self.running_field_key;
            let mut rows = Vec::new();
            let (mut failures, mut first_error, mut nothing_found) = (0, None, 0);
            for outcome in outcomes {
                let Some(index) = model.card_keys.index_of(outcome.key) else { continue };
                let old_value = current_field_value(&model.game.cards()[index], field_key);
                match outcome.suggestion {
                    Ok(Some(value)) if Some(&value) != old_value.as_ref() => rows.push(ReviewRow {
                        key: outcome.key,
                        old_text: old_value.unwrap_or_else(|| "—".to_string()),
                        new_text: value,
                        apply: true,
                    }),
                    Ok(_) => nothing_found += 1,
                    Err(error) => {
                        failures += 1;
                        first_error.get_or_insert(error);
                    }
                }
            }
            rows.sort_by_key(|row| model.card_keys.index_of(row.key));
            let mut summary = format!("\"{field_key}\": {} new values", rows.len());
            if nothing_found > 0 {
                summary += &format!(", {nothing_found} unchanged or not found");
            }
            if failures > 0 {
                summary += &format!(", {failures} failed ({})", first_error.unwrap_or_default());
            }
            if cancelled && done < total {
                summary += &format!(" -- stopped after {done} of {total}");
            }
            if rows.is_empty() {
                frame.ui.toast(&format!("No new values found ({summary})."), None);
                return false;
            }
            self.stage = Stage::Reviewing { rows, summary };
            return true;
        }
        let message = if cancelled { "Stopping...".to_string() } else { format!("{done} of {total} cards") };
        let actions = [DialogAction::text("Cancel").disabled(cancelled)];
        let spec = DialogSpec {
            title: Some("Suggesting Extra Data"),
            width: 420.0,
            actions: &actions,
            dismissible: false,
            ..DialogSpec::default()
        };
        let progress = if total == 0 { None } else { Some(done as f32 / total as f32) };
        let response = frame.dialog("suggestextraprogress", &spec, |frame, x, y, width| {
            frame.text(&message, x, y, TextOptions::new().width(width));
            frame.progress_bar(x, y + 28.0, width, 4.0, progress, None);
            40.0
        });
        if response == DialogResponse::Action(0) {
            job.cancel();
        }
        true
    }

    fn show_review(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        let Stage::Reviewing { rows, summary } = &mut self.stage else { return true };
        let ticked = rows.iter().filter(|row| row.apply && !row.new_text.trim().is_empty()).count();
        let apply_label = format!("Apply {ticked} Value{}", if ticked == 1 { "" } else { "s" });
        let actions = [DialogAction::text("Cancel"), DialogAction::filled(&apply_label).disabled(ticked == 0)];
        let spec = DialogSpec {
            title: Some("Review Suggested Values"),
            width: 720.0,
            actions: &actions,
            ..DialogSpec::default()
        };
        let display_backs = display_card_backs(&model.folder_path, &model.game.card_backs);
        let review_cards = ReviewCards {
            game: &model.game,
            folder_path: &model.folder_path,
            card_keys: &model.card_keys,
            display_backs: &display_backs,
        };
        let response = frame.dialog("suggestextrareview", &spec, |frame, x, y, width| {
            draw_review_rows(frame, "suggestextrareview", "New value", summary, rows, &review_cards, x, y, width)
        });
        match response {
            DialogResponse::Action(1) => {
                let field_key = &self.running_field_key;
                let cards = model.game.cards_mut();
                for row in rows.iter().filter(|row| row.apply && !row.new_text.trim().is_empty()) {
                    if let Some(index) = model.card_keys.index_of(row.key) {
                        set_extra_field(&mut cards[index], field_key, row.new_text.trim());
                    }
                }
                frame.ui.toast(
                    &format!("Updated \"{field_key}\" on {ticked} card{}.", if ticked == 1 { "" } else { "s" }),
                    None,
                );
                false
            }
            DialogResponse::Open | DialogResponse::Enter => true,
            _ => false,
        }
    }
}
