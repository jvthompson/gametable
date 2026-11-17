//! Card View's "Suggest Title" dialogs: choose the source and which cards, watch the progress,
//! then review the suggestions before any title changes (one card's title is filled in directly).

use crate::app::Frame;
use crate::data::image_resolver::resolve_bare_image_path;
use crate::data::settings::keys;
use crate::data::title_suggestion::claude_vision::ClaudeModel;
use crate::data::title_suggestion::local_ocr;
use crate::data::title_suggestion::{
    CardToSuggest, ClaudeAccess, ImageEngine, SuggestionContext, SuggestionJob, TitleSource, is_untitled, suggest_title,
};
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::display_card;
use crate::table::card_render::{CARD_HEIGHT, CardDraw};
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::Weight;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, ToggleOptions};

/// Which cards a run covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestScope {
    /// The card open in the form (or the one card selected).
    ThisCard,
    /// The cards selected in the pool.
    SelectedCards,
    /// Cards whose title is still their ID or image file name.
    UntitledCards,
    AllCards,
}

/// The source picked in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceChoice {
    FromId,
    LocalOcr,
    Claude,
}

impl SourceChoice {
    fn title_source(self) -> TitleSource {
        match self {
            SourceChoice::FromId => TitleSource::FromId,
            SourceChoice::LocalOcr => TitleSource::FromImage(ImageEngine::LocalOcr),
            SourceChoice::Claude => TitleSource::FromImage(ImageEngine::Claude),
        }
    }

    fn setting_value(self) -> &'static str {
        match self {
            SourceChoice::FromId => "id",
            SourceChoice::LocalOcr => "ocr",
            SourceChoice::Claude => "claude",
        }
    }

    fn from_setting_value(value: Option<&str>) -> SourceChoice {
        match value {
            Some("ocr") => SourceChoice::LocalOcr,
            Some("claude") => SourceChoice::Claude,
            _ => SourceChoice::FromId,
        }
    }
}

/// One suggestion in the review list.
#[derive(Debug, Clone)]
struct ReviewRow {
    key: u64,
    old_title: String,
    new_title: String,
    apply: bool,
}

enum Stage {
    Choosing { source: SourceChoice, scope: SuggestScope },
    Running { job: SuggestionJob, single_card: Option<u64> },
    Reviewing { rows: Vec<ReviewRow>, summary: String },
}

/// The open "Suggest Title" flow.
pub struct SuggestTitleFlow {
    stage: Stage,
    /// The card in the form when opened.
    current_card: Option<u64>,
    /// The pool's selection when opened (two or more cards).
    selected_cards: Vec<u64>,
}

/// The saved Claude key: settings first, then the `ANTHROPIC_API_KEY` environment variable.
pub fn claude_api_key(frame: &Frame) -> Option<String> {
    frame
        .settings
        .text(keys::ANTHROPIC_API_KEY)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|key| !key.trim().is_empty()))
}

impl SuggestTitleFlow {
    /// Opens the chooser. `selected_cards` with two or more keys defaults the scope to them.
    pub fn open(frame: &Frame, current_card: Option<u64>, selected_cards: Vec<u64>) -> SuggestTitleFlow {
        let mut source = SourceChoice::from_setting_value(frame.settings.text(keys::SUGGEST_TITLE_SOURCE));
        if source == SourceChoice::Claude && claude_api_key(frame).is_none() {
            source = SourceChoice::FromId;
        }
        let scope = if selected_cards.len() > 1 { SuggestScope::SelectedCards } else { SuggestScope::ThisCard };
        SuggestTitleFlow { stage: Stage::Choosing { source, scope }, current_card, selected_cards }
    }

    /// Draws the current step; returns false once the flow has closed.
    pub fn show(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        match &mut self.stage {
            Stage::Choosing { .. } => self.show_chooser(frame, model),
            Stage::Running { .. } => self.show_progress(frame, model),
            Stage::Reviewing { .. } => self.show_review(frame, model),
        }
    }

    fn scope_choices(&self) -> Vec<Choice<SuggestScope>> {
        let mut choices = Vec::new();
        if self.selected_cards.len() > 1 {
            choices.push(Choice::new(SuggestScope::SelectedCards, format!("Selected ({})", self.selected_cards.len())));
        } else if self.current_card.is_some() {
            choices.push(Choice::new(SuggestScope::ThisCard, "This card"));
        }
        choices.push(Choice::new(SuggestScope::UntitledCards, "Untitled cards"));
        choices.push(Choice::new(SuggestScope::AllCards, "All cards"));
        choices
    }

    fn show_chooser(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        let scope_choices = self.scope_choices();
        let has_key = claude_api_key(frame).is_some();
        let models_missing = local_ocr::models_missing(&local_ocr::models_folder(&frame.paths.app_folder));
        let Stage::Choosing { source, scope } = &mut self.stage else { return true };
        let card_count = indexes_in_scope(model, *scope, self.current_card, &self.selected_cards).len();
        let start_label = if card_count == 1 { "Suggest".to_string() } else { format!("Suggest {card_count} Titles") };
        let actions = [DialogAction::text("Cancel"), DialogAction::filled(&start_label).disabled(card_count == 0)];
        let spec =
            DialogSpec { title: Some("Suggest Title"), width: 520.0, actions: &actions, ..DialogSpec::default() };
        let palette = frame.palette();
        let claude_model = ClaudeModel::from_setting_value(frame.settings.text(keys::CLAUDE_TITLE_MODEL));
        let response = frame.dialog("suggesttitle", &spec, |frame, x, y, width| {
            let mut row_y = y;
            let label = TextOptions::new().medium().width(width);
            frame.text("Read the title from", x, row_y, label);
            row_y += 24.0;
            let source_choices = [
                Choice::new(SourceChoice::FromId, "ID / file name"),
                Choice::new(SourceChoice::LocalOcr, "Image (local)"),
                Choice::new(SourceChoice::Claude, "Image (Claude)"),
            ];
            let picked = frame.segmented("suggesttitle:source", x, row_y, width, &source_choices, *source, None, false);
            *source = if picked == SourceChoice::Claude && !has_key { *source } else { picked };
            row_y += 40.0 + 8.0;
            let note = match *source {
                SourceChoice::FromId => {
                    "Turns IDs like \"059-ursula-power-hungry\" or files like \"LureofPower.jpg\" into titles. Cards \
                     whose ID is only a code are skipped."
                        .to_string()
                }
                SourceChoice::LocalOcr => {
                    let mut note = "Reads the printed title (the card's largest text) with text recognition on this \
                                    computer. Free and offline."
                        .to_string();
                    if models_missing {
                        note += " The first run downloads its models (about 20 MB).";
                    }
                    note
                }
                SourceChoice::Claude => format!(
                    "Sends each card's image to Claude ({}) through the Anthropic API. Most accurate; billed to your \
                     API key's account.",
                    match claude_model {
                        ClaudeModel::Haiku => "Haiku 5.5",
                        ClaudeModel::Opus => "Opus 5.5",
                    }
                ),
            };
            let note = if !has_key && *source != SourceChoice::Claude {
                format!("{note}\nTo use Claude, add your Claude API key in Settings.")
            } else {
                note
            };
            let note_options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);
            frame.text(&note, x, row_y, note_options);
            row_y += frame.text_height(&note, width, 12.0, Weight::Regular) + 16.0;

            frame.text("Cards", x, row_y, label);
            row_y += 24.0;
            *scope = frame.segmented("suggesttitle:scope", x, row_y, width, &scope_choices, *scope, None, false);
            row_y += 40.0 + 8.0;
            let count_text = match card_count {
                0 => "No cards to suggest titles for.".to_string(),
                1 => "1 card.".to_string(),
                count => format!("{count} cards. You'll review the suggestions before anything changes."),
            };
            frame.text(&count_text, x, row_y, note_options);
            row_y + 18.0 - y
        });
        match response {
            DialogResponse::Action(1) => {
                let (source, scope) = (*source, *scope);
                frame.settings.set(keys::SUGGEST_TITLE_SOURCE, source.setting_value());
                self.start(frame, model, source, scope);
                true
            }
            DialogResponse::Open | DialogResponse::Enter => true,
            _ => false,
        }
    }

    fn start(&mut self, frame: &mut Frame, model: &mut EditorModel, source: SourceChoice, scope: SuggestScope) {
        let cards = cards_in_scope(model, scope, self.current_card, &self.selected_cards);
        let context = SuggestionContext {
            game_name: model.game.name.clone(),
            claude: claude_api_key(frame).map(|api_key| ClaudeAccess {
                api_key,
                model: ClaudeModel::from_setting_value(frame.settings.text(keys::CLAUDE_TITLE_MODEL)),
            }),
            ocr_models_folder: local_ocr::models_folder(&frame.paths.app_folder),
        };
        let single_card = (cards.len() == 1).then(|| cards[0].key);
        // From ID on one card needs no thread or progress dialog.
        if source == SourceChoice::FromId && single_card.is_some() {
            let outcome = suggest_title(&cards[0], TitleSource::FromId, &context);
            apply_single(frame, model, cards[0].key, outcome);
            self.stage = Stage::Reviewing { rows: Vec::new(), summary: String::new() };
            return;
        }
        let job = SuggestionJob::start(cards, source.title_source(), context);
        self.stage = Stage::Running { job, single_card };
    }

    fn show_progress(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        let Stage::Running { job, single_card } = &mut self.stage else { return true };
        let finished = job.poll();
        let (done, total) = (job.outcomes.len(), job.total());
        let cancelled = job.is_cancelled();
        if finished || (cancelled && done > 0 && single_card.is_none()) {
            let outcomes = std::mem::take(&mut job.outcomes);
            if let Some(key) = *single_card {
                if let Some(outcome) = outcomes.into_iter().next() {
                    apply_single(frame, model, key, outcome.suggestion);
                }
                return false;
            }
            let mut rows = Vec::new();
            let (mut failures, mut first_error, mut nothing_found) = (0, None, 0);
            for outcome in outcomes {
                let Some(index) = model.card_keys.index_of(outcome.key) else { continue };
                let old_title = model.game.cards()[index].card_title.clone();
                match outcome.suggestion {
                    Ok(Some(title)) if title != old_title => {
                        rows.push(ReviewRow { key: outcome.key, old_title, new_title: title, apply: true })
                    }
                    Ok(_) => nothing_found += 1,
                    Err(error) => {
                        failures += 1;
                        first_error.get_or_insert(error);
                    }
                }
            }
            rows.sort_by_key(|row| model.card_keys.index_of(row.key));
            let mut summary = format!("{} new titles", rows.len());
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
                frame.ui.toast(&format!("No new titles found ({summary})."), None);
                return false;
            }
            self.stage = Stage::Reviewing { rows, summary };
            return true;
        }
        let message = if single_card.is_some() {
            "Reading the card's title...".to_string()
        } else if cancelled {
            "Stopping...".to_string()
        } else {
            format!("{done} of {total} cards")
        };
        let actions = [DialogAction::text("Cancel").disabled(cancelled)];
        let spec = DialogSpec {
            title: Some("Suggesting Titles"),
            width: 420.0,
            actions: &actions,
            dismissible: false,
            ..DialogSpec::default()
        };
        let progress = if single_card.is_some() || total == 0 { None } else { Some(done as f32 / total as f32) };
        let response = frame.dialog("suggestprogress", &spec, |frame, x, y, width| {
            frame.text(&message, x, y, TextOptions::new().width(width));
            frame.progress_bar(x, y + 28.0, width, 4.0, progress, None);
            40.0
        });
        if response == DialogResponse::Action(0) {
            if single_card.is_some() {
                return false;
            }
            job.cancel();
        }
        true
    }

    fn show_review(&mut self, frame: &mut Frame, model: &mut EditorModel) -> bool {
        let Stage::Reviewing { rows, summary } = &mut self.stage else { return true };
        if rows.is_empty() {
            return false;
        }
        let ticked = rows.iter().filter(|row| row.apply).count();
        let apply_label = format!("Apply {ticked} Title{}", if ticked == 1 { "" } else { "s" });
        let actions = [DialogAction::text("Cancel"), DialogAction::filled(&apply_label).disabled(ticked == 0)];
        let spec = DialogSpec {
            title: Some("Review Suggested Titles"),
            width: 720.0,
            actions: &actions,
            ..DialogSpec::default()
        };
        let palette = frame.palette();
        let (_, screen_height) = frame.ui.screen_size();
        let display_backs =
            crate::screens::gamedef::common::display_card_backs(&model.folder_path, &model.game.card_backs);
        let game = &model.game;
        let folder_path = &model.folder_path;
        let card_keys = &model.card_keys;
        let response = frame.dialog("suggestreview", &spec, |frame, x, y, width| {
            let mut row_y = y;
            let summary_options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);
            frame.text(summary, x, row_y, summary_options);
            row_y += frame.text_height(summary, width, 12.0, Weight::Regular) + 8.0;
            let all_ticked = rows.iter().all(|row| row.apply);
            let toggle_label = if all_ticked { "Select None" } else { "Select All" };
            if frame.button(
                toggle_label,
                x - 12.0,
                row_y,
                None,
                Some(32.0),
                &ButtonOptions::variant(ButtonVariant::Text).id("suggestreview:all"),
            ) {
                for row in rows.iter_mut() {
                    row.apply = !all_ticked;
                }
            }
            row_y += 32.0 + 8.0;
            let row_height = 64.0;
            let thumbnail_height = 56.0;
            let thumbnail_width = thumbnail_height * 0.716;
            for row in rows.iter_mut() {
                // Only rows on screen are drawn; long reviews stay quick.
                if row_y + row_height >= 0.0 && row_y <= screen_height {
                    row.apply = frame.checkbox(
                        &format!("suggestreview:{}:apply", row.key),
                        x,
                        row_y + 19.0,
                        row.apply,
                        &ToggleOptions::default(),
                    );
                    if let Some(index) = card_keys.index_of(row.key) {
                        let card = &game.cards()[index];
                        let display = display_card(folder_path, card);
                        let draw = CardDraw::new(
                            Some(&display),
                            true,
                            x + 32.0 + thumbnail_width / 2.0,
                            row_y + row_height / 2.0,
                            &display_backs,
                            game.scenario_card_back_id.as_deref(),
                        )
                        .scale(thumbnail_height / CARD_HEIGHT);
                        frame.draw_card(&draw);
                    }
                    let text_x = x + 32.0 + thumbnail_width + 12.0;
                    let column_width = (x + width - text_x - 8.0) / 2.0;
                    let old_options = TextOptions::new()
                        .width(column_width)
                        .middle_of(row_height)
                        .ellipsis()
                        .color(palette.on_surface_variant);
                    frame.text(&row.old_title, text_x, row_y, old_options);
                    let field_options = TextFieldOptions {
                        height: Some(44.0),
                        label_background: Some(palette.surface_container_high),
                        ..TextFieldOptions::label("New title")
                    };
                    frame.text_field(
                        &format!("suggestreview:{}:title", row.key),
                        text_x + column_width + 8.0,
                        row_y + 10.0,
                        column_width,
                        &mut row.new_title,
                        &field_options,
                    );
                }
                row_y += row_height;
            }
            row_y - y
        });
        match response {
            DialogResponse::Action(1) => {
                let cards = model.game.cards_mut();
                for row in rows.iter().filter(|row| row.apply && !row.new_title.trim().is_empty()) {
                    if let Some(index) = model.card_keys.index_of(row.key) {
                        cards[index].card_title = row.new_title.trim().to_string();
                    }
                }
                frame.ui.toast(&format!("Updated {ticked} card title{}.", if ticked == 1 { "" } else { "s" }), None);
                false
            }
            DialogResponse::Open | DialogResponse::Enter => true,
            _ => false,
        }
    }
}

/// Fills in one card's title, or explains why not.
fn apply_single(frame: &mut Frame, model: &mut EditorModel, key: u64, outcome: Result<Option<String>, String>) {
    match outcome {
        Ok(Some(title)) => {
            if let Some(index) = model.card_keys.index_of(key) {
                model.game.cards_mut()[index].card_title = title;
            }
        }
        Ok(None) => frame.ui.toast("No title found for this card.", None),
        Err(error) => frame.ui.toast(&error, None),
    }
}

/// The card indexes a scope covers, in card order.
fn indexes_in_scope(
    model: &EditorModel,
    scope: SuggestScope,
    current_card: Option<u64>,
    selected_cards: &[u64],
) -> Vec<usize> {
    let cards = model.game.cards();
    let mut indexes: Vec<usize> = match scope {
        SuggestScope::ThisCard => current_card.and_then(|key| model.card_keys.index_of(key)).into_iter().collect(),
        SuggestScope::SelectedCards => selected_cards.iter().filter_map(|&key| model.card_keys.index_of(key)).collect(),
        SuggestScope::UntitledCards => (0..cards.len()).filter(|&index| is_untitled(&cards[index])).collect(),
        SuggestScope::AllCards => (0..cards.len()).collect(),
    };
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

/// The cards a scope covers, with their image paths resolved.
fn cards_in_scope(
    model: &EditorModel,
    scope: SuggestScope,
    current_card: Option<u64>,
    selected_cards: &[u64],
) -> Vec<CardToSuggest> {
    indexes_in_scope(model, scope, current_card, selected_cards)
        .into_iter()
        .map(|index| {
            let card = model.game.cards()[index].clone();
            let image_path = card
                .image_path
                .as_deref()
                .map(|image| resolve_bare_image_path(&model.folder_path, image, card.set_id.as_deref()));
            CardToSuggest { key: model.card_keys.key(index), card, image_path }
        })
        .collect()
}
