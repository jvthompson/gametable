//! "Suggest Title → From ID": turns a card's ID (or, failing that, its image file name) into a
//! readable title. Only useful when the ID or file name holds the card's name, e.g.
//! `059-ursula-power-hungry` → "Ursula Power Hungry" or `LureofPower.jpg` → "Lure of Power".
//! IDs that are codes (`TW-59`, `100723`, `hack1033`, `booster_set_9_birthright_13245`) give
//! no suggestion rather than a nonsense title.

use gametable_core::models::CardDefinition;

/// Small words written in lower case inside a title (but not as its first word).
const LOWER_CASE_SMALL_WORDS: [&str; 18] = [
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "into", "of", "on", "or", "the", "to", "with",
    "that",
];

/// Small words that file names often glue onto the end of the previous word in lower case
/// (`LureofPower`, `WithdrawntoMordor`, `GoldRingthatSauron`).
const GLUED_SMALL_WORDS: [&str; 5] = ["that", "the", "and", "of", "to"];

/// Real words that end in one of [`GLUED_SMALL_WORDS`] and must not be split.
const WORDS_ENDING_IN_A_SMALL_WORD: [&str; 41] = [
    "roof", "proof", "hoof", "aloof", "spoof", "woof", "potato", "tomato", "ditto", "motto", "grotto", "lotto", "alto",
    "auto", "photo", "veto", "lathe", "bathe", "breathe", "clothe", "scythe", "seethe", "soothe", "tithe", "writhe",
    "loathe", "island", "band", "hand", "land", "stand", "sand", "wand", "brand", "grand", "strand", "command",
    "demand", "expand", "bland", "gland",
];

/// The suggested title for `card`: from its ID, else from its image file name.
pub fn suggest_title_from_id(card: &CardDefinition) -> Option<String> {
    title_from_identifier(&card.id)
        .or_else(|| card.image_path.as_deref().and_then(|path| title_from_identifier(&file_stem(path))))
}

/// The file name without folders or extension (`art/LureofPower.jpg` → `LureofPower`).
pub fn file_stem(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].to_string(),
        _ => name.to_string(),
    }
}

/// A readable title from one ID or file stem, or `None` when it looks like a code.
pub fn title_from_identifier(identifier: &str) -> Option<String> {
    let mut words = split_into_words(identifier);
    // A leading collector number (`059-…`) is dropped; numbers anywhere else mean a code.
    while words.first().is_some_and(|word| word.chars().all(|character| character.is_ascii_digit())) {
        words.remove(0);
    }
    if words.is_empty() || words.iter().any(|word| word.chars().any(|character| character.is_ascii_digit())) {
        return None;
    }
    let words: Vec<String> = words.into_iter().flat_map(|word| unglue_small_word(&word)).collect();
    // A single short all-caps word (`TW`, `LE`) is a set code, not a name.
    if words.len() == 1 && words[0].len() <= 3 && words[0].chars().all(|character| character.is_ascii_uppercase()) {
        return None;
    }
    let title_words: Vec<String> =
        words.iter().enumerate().map(|(position, word)| title_case_word(word, position == 0)).collect();
    Some(title_words.join(" "))
}

/// Splits on `-`, `_`, `.`, spaces, lower→upper case changes (`LureofPower` → `Lureof`, `Power`),
/// the end of an upper-case run (`MECCGLure` → `MECCG`, `Lure`) and letter↔digit changes.
fn split_into_words(identifier: &str) -> Vec<String> {
    let characters: Vec<char> = identifier.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (index, &character) in characters.iter().enumerate() {
        if matches!(character, '-' | '_' | '.' | ' ' | '+') {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some(previous) = current.chars().last() {
            let next = characters.get(index + 1).copied();
            let lower_to_upper = previous.is_lowercase() && character.is_uppercase();
            let end_of_upper_run =
                previous.is_uppercase() && character.is_uppercase() && next.is_some_and(|next| next.is_lowercase());
            let letter_digit_change = previous.is_ascii_digit() != character.is_ascii_digit();
            if lower_to_upper || end_of_upper_run || letter_digit_change {
                words.push(std::mem::take(&mut current));
            }
        }
        current.push(character);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// `Lureof` → `Lure`, `of`; leaves real words (`Island`, `Roof`) and short stems alone.
fn unglue_small_word(word: &str) -> Vec<String> {
    let lower = word.to_lowercase();
    if WORDS_ENDING_IN_A_SMALL_WORD.contains(&lower.as_str()) || word.chars().all(|character| character.is_lowercase())
    {
        return vec![word.to_string()];
    }
    for small_word in GLUED_SMALL_WORDS {
        if let Some(stem) = word.strip_suffix(small_word)
            && stem.chars().count() >= 3
            && stem.chars().last().is_some_and(|character| character.is_lowercase())
        {
            return vec![stem.to_string(), small_word.to_string()];
        }
    }
    vec![word.to_string()]
}

/// Capitalises the first letter (keeping the rest, so `MECCG` and `II` survive); small words stay
/// lower case unless they start the title.
fn title_case_word(word: &str, is_first_word: bool) -> String {
    let lower = word.to_lowercase();
    if !is_first_word && LOWER_CASE_SMALL_WORDS.contains(&lower.as_str()) {
        return lower;
    }
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, image: Option<&str>) -> CardDefinition {
        let mut card = CardDefinition::new(id, id);
        card.image_path = image.map(str::to_string);
        card
    }

    #[test]
    fn lorcana_style_ids_become_titles() {
        assert_eq!(title_from_identifier("059-ursula-power-hungry").as_deref(), Some("Ursula Power Hungry"));
        assert_eq!(title_from_identifier("097-chip-quick-thinker").as_deref(), Some("Chip Quick Thinker"));
        assert_eq!(title_from_identifier("080-magical-maneuvers").as_deref(), Some("Magical Maneuvers"));
        assert_eq!(title_from_identifier("012-hounds-of-the-moor").as_deref(), Some("Hounds of the Moor"));
    }

    #[test]
    fn camel_case_file_names_become_titles() {
        assert_eq!(title_from_identifier("LureofPower").as_deref(), Some("Lure of Power"));
        assert_eq!(title_from_identifier("WithdrawntoMordor").as_deref(), Some("Withdrawn to Mordor"));
        assert_eq!(
            title_from_identifier("GoldRingthatSauronFancies").as_deref(),
            Some("Gold Ring that Sauron Fancies")
        );
        assert_eq!(title_from_identifier("LureofMECCG").as_deref(), Some("Lure of MECCG"));
        assert_eq!(title_from_identifier("ThorinII").as_deref(), Some("Thorin II"));
        assert_eq!(title_from_identifier("TheWatersTithe").as_deref(), Some("The Waters Tithe"));
        assert_eq!(title_from_identifier("IslandFortress").as_deref(), Some("Island Fortress"));
        assert_eq!(title_from_identifier("Merry").as_deref(), Some("Merry"));
    }

    #[test]
    fn codes_give_no_suggestion() {
        for code in [
            "TW-59",
            "LE-260",
            "100723",
            "hack1033",
            "booster_set_9_birthright_13245",
            "5th_edition_card_set_158",
            "TW",
        ] {
            assert_eq!(title_from_identifier(code), None, "{code}");
        }
    }

    #[test]
    fn falls_back_to_the_image_file_name() {
        assert_eq!(suggest_title_from_id(&card("TW-59", Some("LureofPower.jpg"))).as_deref(), Some("Lure of Power"));
        assert_eq!(suggest_title_from_id(&card("100723", Some("100723.jpg"))), None);
        assert_eq!(suggest_title_from_id(&card("hack1033", None)), None);
        assert_eq!(file_stem("art\\sub/Wild.Hounds.png"), "Wild.Hounds");
    }
}
