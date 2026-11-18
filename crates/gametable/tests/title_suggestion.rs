//! Card View's "Suggest Title": the button, the chooser, the background run and the review list,
//! driven through real frames (From ID, so no network or text recognition is involved).

use gametable::app::Frame;
use gametable::app::harness::UiHarness;
use gametable::data::settings::keys;
use gametable::screens::gamedef::EditorModel;
use gametable::screens::gamedef::card_view_tab::CardViewTab;
use gametable::screens::gamedef::common::StableKeys;
use gametable::screens::settings_dialog::SettingsDialog;
use gametable_core::models::{CardDefinition, GameDefinition};

fn model_with_cards(ids: &[&str]) -> EditorModel {
    let mut game = GameDefinition::new("test", "Test Game");
    for id in ids {
        game.cards_mut().push(CardDefinition::new(*id, *id));
    }
    let mut card_keys = StableKeys::default();
    card_keys.sync(game.cards().len());
    EditorModel { folder_path: std::env::temp_dir().to_string_lossy().into_owned(), game, card_keys }
}

fn titles(model: &EditorModel) -> Vec<String> {
    model.game.cards().iter().map(|card| card.card_title.clone()).collect()
}

/// Runs frames until `done` holds (the background job needs a moment).
fn frames_until(harness: &mut UiHarness, draw: &mut dyn FnMut(&mut Frame), done: &dyn Fn(&UiHarness) -> bool) {
    for _ in 0..600 {
        harness.frame(|frame| draw(frame));
        if done(harness) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("timed out");
}

#[test]
fn settings_store_the_claude_key_and_model() {
    let mut harness = UiHarness::new();
    // Tall enough that the whole Settings dialog fits without scrolling.
    harness.height = 2400.0;
    let mut dialog = SettingsDialog::new();
    let mut draw = |frame: &mut Frame| {
        dialog.show(frame);
    };
    harness.frame(|frame| draw(frame));
    harness.click_region("settings:anthropicKey", &mut draw);
    harness.type_text("sk-ant-test");
    harness.frame(|frame| draw(frame));
    assert_eq!(harness.settings.text(keys::ANTHROPIC_API_KEY), Some("sk-ant-test"));
    harness.click_region("settings:claudeModel:2", &mut draw);
    assert_eq!(harness.settings.text(keys::CLAUDE_TITLE_MODEL), Some("opus"));
}

#[test]
fn settings_sliders_set_the_ui_scale_and_text_size() {
    let mut harness = UiHarness::new();
    harness.height = 2400.0;
    let mut dialog = SettingsDialog::new();
    let mut draw = |frame: &mut Frame| {
        dialog.show(frame);
    };
    harness.frame(|frame| draw(frame));
    // A click in the middle of each track: UI scale 50%..200% and text size 75%..150%.
    harness.click_region("settings:uiScale", &mut draw);
    harness.frame(|frame| draw(frame));
    assert_eq!(harness.settings.number(keys::UI_SCALE), Some(1.25));
    harness.click_region("settings:textScale", &mut draw);
    // 112.5% rounds to the 5% step above it, stored as a whole percentage.
    assert_eq!(harness.settings.number(keys::TEXT_SCALE), Some(1.15));
}

#[test]
fn card_view_zoom_buttons_resize_the_grid() {
    let mut harness = UiHarness::new();
    let mut model = model_with_cards(&["first", "second"]);
    let mut tab = CardViewTab::new(&model.game);
    let card_region = format!("cv:card:{}", model.card_keys.key(0));
    let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 40.0, 1280.0, 680.0);
    harness.frame(|frame| draw(frame));
    let card_height_before = harness.ui.recorded_regions.as_ref().unwrap()[&card_region].3;
    harness.click_region("cv:zoom:in", &mut draw);
    harness.click_region("cv:zoom:in", &mut draw);
    assert_eq!(harness.settings.number(keys::CARD_VIEW_CARD_ZOOM), Some(1.2));
    harness.frame(|frame| draw(frame));
    let card_height_after = harness.ui.recorded_regions.as_ref().unwrap()[&card_region].3;
    assert!(card_height_after > card_height_before + 20.0, "{card_height_before} -> {card_height_after}");
}

#[test]
fn suggest_title_fills_in_the_current_card() {
    let mut harness = UiHarness::new();
    let mut model = model_with_cards(&["059-ursula-power-hungry", "hack1033"]);
    let mut tab = CardViewTab::new(&model.game);
    tab.select_first(&model);
    {
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 40.0, 1280.0, 680.0);
        harness.frame(|frame| draw(frame));
        let key =
            harness.ui.recorded_regions.as_ref().unwrap().keys().find(|id| id.ends_with(":suggest")).cloned().unwrap();
        harness.click_region(&key, &mut draw);
        assert!(harness.region_center("suggesttitle:a2").is_some(), "the chooser is open");
        harness.click_region("suggesttitle:a2", &mut draw);
    }
    assert_eq!(titles(&model), ["Ursula Power Hungry", "hack1033"]);
}

#[test]
fn suggest_titles_for_all_cards_goes_through_the_review_list() {
    let mut harness = UiHarness::new();
    let mut model = model_with_cards(&["059-ursula-power-hungry", "hack1033", "080-magical-maneuvers"]);
    let mut tab = CardViewTab::new(&model.game);
    tab.select_first(&model);
    let third_card_key = model.card_keys.key(2);
    {
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 40.0, 1280.0, 680.0);
        harness.frame(|frame| draw(frame));
        let key =
            harness.ui.recorded_regions.as_ref().unwrap().keys().find(|id| id.ends_with(":suggest")).cloned().unwrap();
        harness.click_region(&key, &mut draw);
        // Cards: This card / Untitled cards / All cards.
        harness.click_region("suggesttitle:scope:3", &mut draw);
        harness.click_region("suggesttitle:a2", &mut draw);
        let review_apply = format!("suggestreview:{third_card_key}:apply");
        frames_until(&mut harness, &mut draw, &|harness| harness.region_center(&review_apply).is_some());
        // Untick the third card, then apply the rest.
        harness.click_region(&review_apply, &mut draw);
        harness.click_region("suggestreview:a2", &mut draw);
    }
    assert_eq!(titles(&model), ["Ursula Power Hungry", "hack1033", "080-magical-maneuvers"]);
}
