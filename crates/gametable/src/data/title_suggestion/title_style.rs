//! How a game writes its titles, learned from the titles its cards already have, so suggestions
//! match: what joins a name to its subtitle ("Ariel, On Human Legs" in Lorcana), a few real
//! titles as examples for Claude, and turning titles printed in capitals into title case.

/// What goes between a card's name and its subtitle (two lines in a title area).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineSeparator {
    #[default]
    Space,
    Comma,
    Dash,
    Colon,
}

impl LineSeparator {
    /// The value stored in settings (per game, once the user has run with it).
    pub fn setting_value(self) -> &'static str {
        match self {
            LineSeparator::Space => "space",
            LineSeparator::Comma => "comma",
            LineSeparator::Dash => "dash",
            LineSeparator::Colon => "colon",
        }
    }

    pub fn from_setting_value(value: &str) -> Option<LineSeparator> {
        LineSeparator::ALL.into_iter().find(|separator| separator.setting_value() == value)
    }

    pub const ALL: [LineSeparator; 4] =
        [LineSeparator::Space, LineSeparator::Comma, LineSeparator::Dash, LineSeparator::Colon];

    /// The text put between the two parts.
    pub fn joiner(self) -> &'static str {
        match self {
            LineSeparator::Space => " ",
            LineSeparator::Comma => ", ",
            LineSeparator::Dash => " - ",
            LineSeparator::Colon => ": ",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineSeparator::Space => "Space",
            LineSeparator::Comma => "Comma",
            LineSeparator::Dash => "Dash",
            LineSeparator::Colon => "Colon",
        }
    }
}

/// The house style suggestions follow.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TitleStyle {
    pub line_separator: LineSeparator,
    /// A few of the game's real titles, shown to Claude as examples.
    pub examples: Vec<String>,
}

/// At least this many titles must use a separator before it counts as the game's style.
const MINIMUM_TITLES_WITH_SEPARATOR: usize = 3;
/// How many example titles Claude is shown.
const EXAMPLE_COUNT: usize = 5;
/// Example titles longer than this are skipped (long titles are usually oddities).
const MAXIMUM_EXAMPLE_LENGTH: usize = 60;

/// The separator a game's titles use between a name and a subtitle: of the titles that use one
/// at all, the separator used by at least 3 and at least twice as often as any other; otherwise a
/// space. (Titles without one don't count: a library often has whole sets titled from card IDs.)
pub fn detect_line_separator<'a>(titles: impl IntoIterator<Item = &'a str>) -> LineSeparator {
    let titles: Vec<&str> = titles.into_iter().map(str::trim).filter(|title| !title.is_empty()).collect();
    let mut counts: Vec<(LineSeparator, usize)> = LineSeparator::ALL[1..]
        .iter()
        .map(|&separator| {
            let joiner = separator.joiner();
            (separator, titles.iter().filter(|title| title.contains(joiner)).count())
        })
        .collect();
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let (best, best_count) = counts[0];
    let runner_up_count = counts[1].1;
    let common_enough = best_count >= MINIMUM_TITLES_WITH_SEPARATOR;
    let clearly_ahead = best_count >= runner_up_count * 2;
    if common_enough && clearly_ahead { best } else { LineSeparator::Space }
}

/// Up to five distinct, reasonably short titles, those using the separator first.
pub fn pick_example_titles<'a>(titles: impl IntoIterator<Item = &'a str>, separator: LineSeparator) -> Vec<String> {
    let mut candidates: Vec<&str> = Vec::new();
    for title in titles.into_iter().map(str::trim) {
        if !title.is_empty() && title.len() <= MAXIMUM_EXAMPLE_LENGTH && !candidates.contains(&title) {
            candidates.push(title);
        }
    }
    let uses_separator = |title: &&str| separator != LineSeparator::Space && title.contains(separator.joiner());
    let (with, without): (Vec<&str>, Vec<&str>) = candidates.into_iter().partition(uses_separator);
    with.into_iter().chain(without).take(EXAMPLE_COUNT).map(str::to_string).collect()
}

/// Small words kept lower case inside a title (but not at the start of a title or subtitle).
const MINOR_WORDS: &[&str] =
    &["a", "an", "the", "and", "or", "nor", "but", "of", "in", "on", "to", "for", "at", "by", "with", "from", "as"];

/// `THE BIG BANG` → `The Big Bang`: a title printed all in capitals becomes title case (small
/// words like "of" lower case, except first, or first after a `,` `:` or ` - `). A title with
/// any lower-case letter is left as it is, so `The BIG Bang` stays.
pub fn title_case_if_all_caps(text: &str) -> String {
    let has_lower = text.chars().any(char::is_lowercase);
    let has_upper = text.chars().any(char::is_uppercase);
    if has_lower || !has_upper {
        return text.to_string();
    }
    let mut starts_part = true;
    text.split(' ')
        .map(|word| {
            let lower = word.to_lowercase();
            let bare = lower.trim_matches(|character: char| !character.is_alphanumeric());
            let keep_lower = !starts_part && MINOR_WORDS.contains(&bare);
            starts_part = word.ends_with(',') || word.ends_with(':') || matches!(word, "-" | "–" | "—");
            if keep_lower || word.is_empty() { lower } else { capitalize_first_letter(&lower) }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Upper-cases the first letter, skipping any leading punctuation (`"hello` → `"Hello`).
fn capitalize_first_letter(word: &str) -> String {
    let mut result = String::with_capacity(word.len());
    let mut done = false;
    for character in word.chars() {
        if !done && character.is_alphabetic() {
            result.extend(character.to_uppercase());
            done = true;
        } else {
            result.push(character);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_separator_most_titles_use_is_learned() {
        let lorcana =
            ["Ariel, On Human Legs", "Ursula, Power Hungry", "Hades, King of Olympus", "Be Our Guest", "Lantern"];
        assert_eq!(detect_line_separator(lorcana), LineSeparator::Comma);
        let dashes = ["Spider-Man - Hero", "Venom - Symbiote", "Thor - Odinson", "Hulk"];
        assert_eq!(detect_line_separator(dashes), LineSeparator::Dash);
        // Too few, or no clear winner: a plain space.
        assert_eq!(detect_line_separator(["Lure of Power", "Ghost, Then Gone", "Sithicus"]), LineSeparator::Space);
        let mixed = ["A, B", "C, D", "E, F", "G: H", "I: J", "K: L"];
        assert_eq!(detect_line_separator(mixed), LineSeparator::Space);
        // Titles without a separator (whole sets titled from IDs) don't outvote the ones with.
        let mut many: Vec<&str> = vec!["Bashful Hopeless Romantic"; 300];
        many.extend(["A, B", "C, D", "E, F", "G - H"]);
        assert_eq!(detect_line_separator(many), LineSeparator::Comma);
    }

    #[test]
    fn examples_prefer_titles_with_the_separator() {
        let titles = ["Be Our Guest", "Ariel, On Human Legs", "Be Our Guest", "Ursula, Power Hungry"];
        assert_eq!(
            pick_example_titles(titles, LineSeparator::Comma),
            ["Ariel, On Human Legs", "Ursula, Power Hungry", "Be Our Guest"]
        );
        assert_eq!(pick_example_titles(["A", "B", "C", "D", "E", "F"], LineSeparator::Space).len(), 5);
    }

    #[test]
    fn titles_in_capitals_become_title_case() {
        assert_eq!(title_case_if_all_caps("THE BIG BANG"), "The Big Bang");
        assert_eq!(title_case_if_all_caps("The BIG Bang"), "The BIG Bang");
        assert_eq!(title_case_if_all_caps("LURE OF POWER"), "Lure of Power");
        assert_eq!(title_case_if_all_caps("ARIEL, ON HUMAN LEGS"), "Ariel, On Human Legs");
        assert_eq!(title_case_if_all_caps("THE LORD - OF THE RINGS"), "The Lord - Of the Rings");
        assert_eq!(title_case_if_all_caps("\"QUOTED\" TITLE"), "\"Quoted\" Title");
        assert_eq!(title_case_if_all_caps("123"), "123");
    }
}
