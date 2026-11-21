//! "Suggest Extra Data" in the game definition editor's Card View: reads one extra field's value
//! (a cost, a strength, a set number...) from each card's picture with local text recognition or
//! Claude, using the same background job as "Suggest Title".

use std::collections::HashSet;

use gametable_core::json::{JsonValue, encode_compact};
use gametable_core::models::{CardDefinition, GameDefinition};

use super::local_ocr::TextLineCandidate;

/// The most example values from other cards given to Claude.
const MAXIMUM_EXAMPLE_VALUES: usize = 8;

/// An extra field's value as the editor shows it (whole numbers without a decimal point).
pub fn extra_value_text(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) if number.fract() == 0.0 && number.abs() < 1e15 => format!("{}", *number as i64),
        JsonValue::Number(number) => number.to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        other => encode_compact(other).unwrap_or_default(),
    }
}

/// The card's own spelling of `field_key` (compared ignoring case), when it has that field.
pub fn existing_field_key<'card>(card: &'card CardDefinition, field_key: &str) -> Option<&'card String> {
    card.extra_fields.keys().find(|key| key.eq_ignore_ascii_case(field_key))
}

/// The card's current value for `field_key` (ignoring case), as text.
pub fn current_field_value(card: &CardDefinition, field_key: &str) -> Option<String> {
    let key = existing_field_key(card, field_key)?;
    card.extra_fields.get(key).map(extra_value_text)
}

/// Sets the field, updating an entry whose key differs only in case rather than adding a second.
pub fn set_extra_field(card: &mut CardDefinition, field_key: &str, value: &str) {
    let key = existing_field_key(card, field_key).cloned().unwrap_or_else(|| field_key.to_string());
    card.extra_fields.insert(key, value.to_string());
}

/// Distinct non-empty values other cards have for the field, so Claude can match their format.
pub fn pick_example_values(game: &GameDefinition, field_key: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    game.cards()
        .iter()
        .filter_map(|card| current_field_value(card, field_key))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .take(MAXIMUM_EXAMPLE_VALUES)
        .collect()
}

/// Claude's prompt for one card's field.
pub fn extra_field_instructions(
    game_name: &str,
    card: &CardDefinition,
    field_key: &str,
    area_only: bool,
    example_values: &[String],
    custom_instructions: &str,
) -> String {
    let card_name = if card.card_title.trim().is_empty() { card.id.as_str() } else { card.card_title.as_str() };
    let picture = if area_only {
        format!(
            "This picture is the part of a card from the game \"{game_name}\" (the card \"{card_name}\", ID \"{}\") \
             where its \"{field_key}\" is printed.",
            card.id
        )
    } else {
        format!("This picture is a card from the game \"{game_name}\" (the card \"{card_name}\", ID \"{}\").", card.id)
    };
    let mut text = format!(
        "{picture} Read the card's \"{field_key}\" value exactly as it is printed, keeping its punctuation; write \
         numbers as digits. Give only the value, without the field's name. If the card shows no \"{field_key}\" \
         value, set found to false and value to an empty string."
    );
    if !example_values.is_empty() {
        let quoted: Vec<String> = example_values.iter().map(|value| format!("\"{value}\"")).collect();
        text.push_str(&format!(
            " For reference, other cards in this game have \"{field_key}\" values like {}; match their format.",
            quoted.join(", ")
        ));
    }
    let custom_instructions = custom_instructions.trim();
    if !custom_instructions.is_empty() {
        text.push_str(&format!(" Additional instructions for this field: {custom_instructions}"));
    }
    text
}

/// The text recognised inside a field area: every line, top to bottom, joined with spaces. Unlike
/// titles, numbers and symbols are kept ("3", "2/4", "+1").
pub fn join_value_lines(candidates: impl IntoIterator<Item = TextLineCandidate>) -> Option<String> {
    let mut lines: Vec<TextLineCandidate> = candidates.into_iter().collect();
    lines.sort_by(|first, second| first.top.total_cmp(&second.top));
    let joined = lines
        .iter()
        .map(|line| tidy_value_line(&line.text))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// Collapses spaces and drops stray marks text recognition puts at the ends of a line.
fn tidy_value_line(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_matches(|character: char| "•·|_~`".contains(character) || character.is_whitespace()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, top: f32) -> TextLineCandidate {
        TextLineCandidate { text: text.to_string(), top, height: 10.0 }
    }

    #[test]
    fn field_area_lines_join_top_to_bottom_and_keep_numbers() {
        assert_eq!(join_value_lines(vec![line("Ranged", 30.0), line(" • 3 ", 5.0)]).as_deref(), Some("3 Ranged"));
        assert_eq!(join_value_lines(vec![line("2/4", 0.0)]).as_deref(), Some("2/4"));
        assert_eq!(join_value_lines(vec![line("+1", 0.0)]).as_deref(), Some("+1"));
        assert_eq!(join_value_lines(vec![line(" | ", 0.0)]), None);
        assert_eq!(join_value_lines(Vec::new()), None);
    }

    #[test]
    fn fields_are_matched_ignoring_case() {
        let mut card = CardDefinition::new("a", "A");
        card.extra_fields.insert("Cost", 3.0);
        assert_eq!(current_field_value(&card, "cost").as_deref(), Some("3"));
        set_extra_field(&mut card, "COST", "4");
        assert_eq!(card.extra_fields.len(), 1);
        assert_eq!(current_field_value(&card, "Cost").as_deref(), Some("4"));
        set_extra_field(&mut card, "Power", "2");
        assert_eq!(card.extra_fields.keys().cloned().collect::<Vec<_>>(), vec!["Cost", "Power"]);
    }

    #[test]
    fn example_values_are_distinct_and_capped() {
        let mut game = GameDefinition::default();
        let mut cards = Vec::new();
        for index in 0..20 {
            let mut card = CardDefinition::new(format!("c{index}"), "Card");
            card.extra_fields.insert("cost", (index % 10) as f64);
            cards.push(card);
        }
        cards.push(CardDefinition::new("none", "No Cost"));
        *game.cards_mut() = cards;
        let examples = pick_example_values(&game, "Cost");
        assert_eq!(examples, vec!["0", "1", "2", "3", "4", "5", "6", "7"]);
    }

    #[test]
    fn the_prompt_names_the_field_and_examples() {
        let card = CardDefinition::new("059-ursula", "Ursula, Power Hungry");
        let text = extra_field_instructions("Lorcana", &card, "Cost", true, &["3".into(), "7".into()], "");
        assert!(text.contains("where its \"Cost\" is printed"), "{text}");
        assert!(text.contains("\"Ursula, Power Hungry\"") && text.contains("\"059-ursula\""), "{text}");
        assert!(text.contains("values like \"3\", \"7\""), "{text}");
        let whole = extra_field_instructions("Lorcana", &card, "Cost", false, &[], "  ");
        assert!(whole.contains("This picture is a card") && !whole.contains("For reference"), "{whole}");
        assert!(!whole.contains("Additional instructions"), "{whole}");
        let custom =
            extra_field_instructions("Lorcana", &card, "Ink", false, &[], " Use \"Inkable\" or \"Uninkable\". ");
        assert!(
            custom.ends_with("Additional instructions for this field: Use \"Inkable\" or \"Uninkable\"."),
            "{custom}"
        );
    }
}
