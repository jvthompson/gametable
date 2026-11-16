//! Replays scripted action sequences and compares every intermediate state with golden output
//! from CardTable's original `table_actions.lua` (see `tools/lua_golden/table_actions_golden.lua`).
//! Regenerate with `tools/lua_golden/run.sh table_actions`.

use std::path::Path;

use gametable_core::json::{self, JsonValue};
use gametable_core::models::*;
use gametable_core::table_actions::*;
use gametable_core::util::ParkMillerRandom;

/// Typed access to one step's argument list (JSON `null` means "absent").
struct StepArguments<'a>(&'a [JsonValue]);

impl<'a> StepArguments<'a> {
    fn get(&self, index: usize) -> Option<&'a JsonValue> {
        self.0.get(index).filter(|value| **value != JsonValue::Null)
    }
    fn text(&self, index: usize) -> &'a str {
        self.get(index).and_then(JsonValue::as_str).unwrap_or_else(|| panic!("argument {index} must be text"))
    }
    fn optional_text(&self, index: usize) -> Option<&'a str> {
        self.get(index).map(|value| value.as_str().expect("text or null"))
    }
    fn number(&self, index: usize) -> f64 {
        self.get(index).and_then(JsonValue::as_f64).unwrap_or_else(|| panic!("argument {index} must be a number"))
    }
    fn integer(&self, index: usize) -> i64 {
        self.number(index) as i64
    }
    fn flag(&self, index: usize) -> bool {
        self.get(index).is_some_and(JsonValue::is_exactly_true)
    }
    fn color(&self, index: usize) -> u32 {
        self.number(index) as u32
    }
    fn text_list(&self, index: usize) -> Vec<String> {
        let items = self.get(index).and_then(JsonValue::as_array).expect("list argument");
        items.iter().map(|item| item.as_str().expect("text item").to_string()).collect()
    }
    fn size_override(&self, index: usize) -> Option<WidgetSizeOverride> {
        match self.get(index) {
            None => None,
            Some(JsonValue::Number(multiplier)) => Some(WidgetSizeOverride::Multiplier(*multiplier)),
            Some(JsonValue::String(text)) if text == "native" => Some(WidgetSizeOverride::NativeImageSize),
            Some(other) => panic!("bad size override {other:?}"),
        }
    }
    fn widget_kind(&self, index: usize) -> BoardWidgetKind {
        BoardWidgetKind::from_json_str(self.text(index)).expect("widget kind")
    }
    fn search_target(&self, index: usize) -> SearchTargetType {
        SearchTargetType::from_json_str(self.text(index)).expect("search target type")
    }
    fn list_of<T>(&self, index: usize, parse: impl Fn(&JsonValue) -> Result<T, ModelError>) -> Vec<T> {
        let items = self.get(index).and_then(JsonValue::as_array).expect("list argument");
        items.iter().map(|item| parse(item).expect("valid model")).collect()
    }
}

/// Applies one scripted action; returns the new state and, for draws, the drawn ids.
fn apply_step(state: &TableState, action: &str, args: &StepArguments) -> (TableState, Option<Vec<String>>) {
    let no_draw = |state: TableState| (state, None);
    match action {
        "moveCard" => no_draw(move_card(state, args.text(0), args.number(1), args.number(2))),
        "moveStack" => no_draw(move_stack(state, args.text(0), args.number(1), args.number(2))),
        "rotateStack" => no_draw(rotate_stack(state, args.text(0), args.flag(1), args.number(2))),
        "sendToBack" => no_draw(send_to_back(state, args.text(0))),
        "flipCard" => no_draw(flip_card(state, args.text(0))),
        "giveCard" => no_draw(give_card(state, args.text(0), args.optional_text(1))),
        "stackCard" => no_draw(stack_card(state, args.text(0), args.text(1))),
        "moveGroup" => no_draw(move_group(state, args.text(0), &args.text_list(1), args.number(2), args.number(3))),
        "moveToHand" => no_draw(move_to_hand(state, args.text(0), args.text(1))),
        "reorderHand" => no_draw(reorder_hand(state, args.text(0), args.optional_text(1), args.integer(2))),
        "drawCard" => {
            let result = draw_card(state, args.text(0), args.text(1), args.integer(2));
            (result.state, Some(result.drawn_instance_ids))
        }
        "drawFromZone" => {
            let result = draw_from_zone(state, args.text(0), args.optional_text(1), args.text(2), args.integer(3));
            (result.state, Some(result.drawn_instance_ids))
        }
        "returnToZone" => no_draw(return_to_zone(
            state,
            args.text(0),
            args.text(1),
            args.optional_text(2),
            args.flag(3),
            args.flag(4),
        )),
        "shuffleZone" => no_draw(shuffle_zone(
            state,
            args.text(0),
            args.optional_text(1),
            &mut ParkMillerRandom::with_seed(args.integer(2)),
        )),
        "shufflePile" => no_draw(shuffle_pile(state, args.text(0), &mut ParkMillerRandom::with_seed(args.integer(1)))),
        "startSearch" => {
            no_draw(start_search(state, args.text(0), args.search_target(1), args.text(2), args.optional_text(3)))
        }
        "stopSearch" => no_draw(stop_search(state, args.text(0))),
        "createWidget" => no_draw(create_widget(
            state,
            args.text(0),
            args.widget_kind(1),
            args.number(2),
            args.number(3),
            args.optional_text(4),
            args.optional_text(5),
        )),
        "addWidget" => {
            no_draw(add_widget(state, BoardWidgetInstance::from_json(args.get(0).unwrap()).expect("widget")))
        }
        "createArrow" => no_draw(create_arrow(
            state,
            args.text(0),
            args.number(1),
            args.number(2),
            args.number(3),
            args.number(4),
            args.optional_text(5),
        )),
        "attachWidgetToCard" => {
            no_draw(attach_widget_to_card(state, args.text(0), args.text(1), args.number(2), args.number(3)))
        }
        "moveWidget" => no_draw(move_widget(state, args.text(0), args.number(1), args.number(2))),
        "setWidgetValue" => no_draw(set_widget_value(state, args.text(0), args.number(1))),
        "setWidgetColors" => no_draw(set_widget_colors(state, args.text(0), args.color(1), args.color(2))),
        "setWidgetText" => no_draw(set_widget_text(state, args.text(0), args.text(1), args.color(2))),
        "setWidgetSize" => no_draw(set_widget_size(state, args.text(0), args.size_override(1))),
        "rotateWidget" => no_draw(rotate_widget(state, args.text(0), args.flag(1), args.number(2))),
        "flipWidget" => no_draw(flip_widget(state, args.text(0))),
        "sendWidgetToBack" => no_draw(send_widget_to_back(state, args.text(0))),
        "duplicateWidget" => {
            no_draw(duplicate_widget(state, args.text(0), args.text(1), args.number(2), args.number(3)))
        }
        "addWidgets" => no_draw(add_widgets(state, args.list_of(0, BoardWidgetInstance::from_json))),
        "revealBagToken" => no_draw(reveal_bag_token(state, args.text(0), args.text(1), args.text(2))),
        "resolveBagToken" => no_draw(resolve_bag_token(state, args.text(0), args.text(1), args.flag(2))),
        "placeBagTokenOnTable" => no_draw(place_bag_token_on_table(
            state,
            args.text(0),
            args.text(1),
            args.text(2),
            args.number(3),
            args.number(4),
            args.optional_text(5),
            args.size_override(6),
        )),
        "returnImageTokenToBag" => no_draw(return_image_token_to_bag(state, args.text(0), args.text(1), args.flag(2))),
        "setBagRemaining" => no_draw(set_bag_remaining(state, args.text(0), &args.text_list(1))),
        "resetBag" => no_draw(reset_bag(state, args.text(0), &args.text_list(1))),
        "deleteWidget" => no_draw(delete_widget(state, args.text(0))),
        "dealNewCards" => no_draw(deal_new_cards(state, args.list_of(0, CardInstance::from_json))),
        other => panic!("table action {other} is not mapped in the golden test"),
    }
}

#[test]
fn scripted_actions_match_the_original_lua_step_by_step() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/table_actions");
    let mut script_count = 0;
    for entry in std::fs::read_dir(fixtures.join("inputs")).unwrap() {
        let path = entry.unwrap().path();
        let base_name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let script = json::decode_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let expected_text =
            std::fs::read_to_string(fixtures.join("expected").join(format!("{base_name}.expected.txt"))).unwrap();
        let expected_lines: Vec<&str> = expected_text.split('\n').collect();

        let mut state = TableState::from_json(script.get("initialState").unwrap()).unwrap();
        let steps = script.get("steps").and_then(JsonValue::as_array).unwrap();
        assert_eq!(steps.len(), expected_lines.len(), "{base_name}: step count differs from golden output");

        for (step_index, (step, expected_line)) in steps.iter().zip(expected_lines).enumerate() {
            let action = step.get("action").and_then(JsonValue::as_str).unwrap();
            let arguments = step.get("args").and_then(JsonValue::as_array).unwrap();
            let (next_state, drawn_ids) = apply_step(&state, action, &StepArguments(arguments));
            let mut actual_line = json::encode_compact(&next_state.to_json()).unwrap();
            if let Some(drawn_ids) = drawn_ids {
                actual_line.push('|');
                actual_line.push_str(&json::encode_compact(&JsonValue::from(drawn_ids)).unwrap());
            }
            assert_eq!(
                actual_line,
                expected_line,
                "{base_name}: step {} ({action}) differs from the original",
                step_index + 1
            );
            state = next_state;
        }
        script_count += 1;
    }
    assert!(script_count > 0);
}
