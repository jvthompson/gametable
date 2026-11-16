//! File-format compatibility check over an existing CardTable library. Port of
//! `tools/compat_check/main.lua` (minus image decoding, which needs the SDL client).
//!
//! Usage:
//!   cargo run -p gametable-core --example compat_check -- <libraryRoot> [--resaved-output <dir>]
//!
//! `<libraryRoot>` contains `game_library/` and `deck_library/`. Every game definition, built-in
//! deck, saved game and library deck is loaded and re-encoded; the report says whether the re-saved
//! text is identical to the file, identical apart from Dart-style `1.0` numbers, or different.
//!
//! The library is only ever read. With `--resaved-output`, the re-encoded files are written under
//! that directory (mirroring their relative paths) so they can be diffed against another
//! implementation's output; the directory must not be inside the library.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gametable_core::json::{self, JsonValue};
use gametable_core::models::{
    CARD_BACK_FILE_SUFFIX, DeckConfig, GameDefinition, ModelError, SavedGame, UNIQUE_CARD_BACK_ID,
};

struct Report {
    lines: Vec<String>,
    problem_count: usize,
    resaved_output: Option<PathBuf>,
    library_root: PathBuf,
}

impl Report {
    fn say(&mut self, line: impl Into<String>) {
        let line = line.into();
        println!("{line}");
        self.lines.push(line);
    }

    fn problem(&mut self, line: impl AsRef<str>) {
        self.problem_count += 1;
        self.say(format!("  PROBLEM: {}", line.as_ref()));
    }

    /// Writes a re-encoded file (or an error note) under the output directory.
    fn write_resaved(&self, original_path: &Path, contents: Result<&str, String>) {
        let Some(output_root) = &self.resaved_output else { return };
        let relative = original_path.strip_prefix(&self.library_root).expect("library file");
        let mut target = output_root.join(relative);
        let text = match contents {
            Ok(text) => text.to_string(),
            Err(error) => {
                target.set_extension("error.txt");
                error
            }
        };
        std::fs::create_dir_all(target.parent().unwrap()).expect("create output directory");
        std::fs::write(&target, text).expect("write re-saved file");
    }
}

// ---------------------------------------------------------------------------
// Comparing re-saved text with the original file
// ---------------------------------------------------------------------------

fn normalize_line_endings_and_trailing_space(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end().to_string()
}

/// Dart wrote whole doubles as `1.0`; Lua (and this port) write `1`. Drops `.0` after a digit
/// when followed by `,`, whitespace, `]` or `}` (the original's `stripDotZero`).
fn strip_dart_dot_zero(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        let is_dot_zero_after_digit = index > 0
            && bytes[index - 1].is_ascii_digit()
            && bytes[index] == b'.'
            && bytes.get(index + 1) == Some(&b'0')
            && bytes
                .get(index + 2)
                .is_some_and(|next| matches!(next, b',' | b']' | b'}') || next.is_ascii_whitespace());
        if is_dot_zero_after_digit {
            index += 2;
            continue;
        }
        let character_length = text[index..].chars().next().unwrap().len_utf8();
        output.push_str(&text[index..index + character_length]);
        index += character_length;
    }
    output
}

/// Paths where two JSON values differ, ignoring object key order (at most 8).
fn structural_differences(a: &JsonValue, b: &JsonValue, path: &str, found: &mut Vec<String>) {
    if found.len() >= 8 {
        return;
    }
    match (a, b) {
        (JsonValue::Object(left), JsonValue::Object(right)) => {
            let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child_path = format!("{path}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(l), Some(r)) => structural_differences(l, r, &child_path, found),
                    (l, r) => found.push(format!("{child_path}: {} vs {}", describe(l), describe(r))),
                }
            }
        }
        (JsonValue::Array(left), JsonValue::Array(right)) => {
            for index in 0..left.len().max(right.len()) {
                let child_path = format!("{path}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(l), Some(r)) => structural_differences(l, r, &child_path, found),
                    (l, r) => found.push(format!("{child_path}: {} vs {}", describe(l), describe(r))),
                }
            }
        }
        _ if a != b => found.push(format!("{path}: {} vs {}", describe(Some(a)), describe(Some(b)))),
        _ => {}
    }
}

fn describe(value: Option<&JsonValue>) -> String {
    match value {
        None => "(absent)".to_string(),
        Some(value) => {
            let encoded = json::encode_compact(value).unwrap_or_default();
            if encoded.len() > 60 { format!("{}…", &encoded[..encoded.floor_char_boundary(60)]) } else { encoded }
        }
    }
}

fn compare_with_original(report: &mut Report, label: &str, original: &str, resaved: &str) -> String {
    let (original_normalized, resaved_normalized) =
        (normalize_line_endings_and_trailing_space(original), normalize_line_endings_and_trailing_space(resaved));
    if original_normalized == resaved_normalized {
        return "identical".to_string();
    }
    if strip_dart_dot_zero(&original_normalized) == strip_dart_dot_zero(&resaved_normalized) {
        return "identical except number formatting (1.0 vs 1)".to_string();
    }
    let (Ok(original_value), Ok(resaved_value)) = (json::decode(original), json::decode(resaved)) else {
        report.problem(format!("{label}: could not decode for comparison"));
        return "UNCOMPARABLE".to_string();
    };
    // Old files: the single cardBackImagePath becomes a one-entry cardBacks list on save.
    if original_value.get("cardBackImagePath").is_some()
        && original_value.get("cardBacks").is_none()
        && let Some(card_backs) = resaved_value.get("cardBacks")
    {
        let mut migrated = original_value.clone();
        let object = migrated.as_object_mut().unwrap();
        object.remove("cardBackImagePath");
        object.insert("cardBacks", card_backs.clone());
        let mut differences = Vec::new();
        structural_differences(&migrated, &resaved_value, "$", &mut differences);
        if differences.is_empty() {
            return "legacy cardBackImagePath migrated to cardBacks (the original does the same on save)".to_string();
        }
    }
    let mut differences = Vec::new();
    structural_differences(&original_value, &resaved_value, "$", &mut differences);
    if differences.is_empty() {
        return "same data, different layout/key order".to_string();
    }
    report.problem(format!("{label} re-saves differently:"));
    for difference in differences {
        report.say(format!("      {difference}"));
    }
    "DATA DIFFERS".to_string()
}

// ---------------------------------------------------------------------------
// Library scanning (same rules as src/data/game_loader.lua)
// ---------------------------------------------------------------------------

fn sorted_entries(directory: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(directory) {
        Ok(read) => read.filter_map(Result::ok).map(|entry| entry.path()).collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    entries
}

fn file_name_lowercase(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn json_files_in(directory: &Path) -> Vec<PathBuf> {
    sorted_entries(directory)
        .into_iter()
        .filter(|path| path.is_file() && file_name_lowercase(path).ends_with(".json"))
        .collect()
}

fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).map_err(|error| error.to_string())
}

/// `<card back image stem>_BACK<ext>` next to the card image.
fn unique_back_path(card_image_path: &Path) -> PathBuf {
    let stem = card_image_path.file_stem().unwrap_or_default().to_string_lossy();
    let file_name = match card_image_path.extension() {
        Some(extension) => format!("{stem}{CARD_BACK_FILE_SUFFIX}.{}", extension.to_string_lossy()),
        None => format!("{stem}{CARD_BACK_FILE_SUFFIX}"),
    };
    card_image_path.with_file_name(file_name)
}

fn check_game_folder(report: &mut Report, folder: &Path) -> Option<GameDefinition> {
    let gamedef_path = sorted_entries(folder)
        .into_iter()
        .find(|path| path.is_file() && file_name_lowercase(path) == "gamedef.json")?;
    let raw = match read_text(&gamedef_path) {
        Ok(raw) => raw,
        Err(error) => {
            report.problem(format!("could not read {}: {error}", gamedef_path.display()));
            return None;
        }
    };
    let game = match json::decode(&raw)
        .map_err(|error| error.to_string())
        .and_then(|value| GameDefinition::from_json(&value).map_err(|error| error.to_string()))
    {
        Ok(game) => game,
        Err(error) => {
            report.problem(format!("could not load {}: {error}", gamedef_path.display()));
            report.write_resaved(&gamedef_path, Err(error));
            return None;
        }
    };
    report.say(format!(
        "\n-- {} ({}): {} cards, {} sets, {} zones, {} tag groups, {} card backs, {} scenarios, {} tokens",
        game.name,
        game.id,
        game.cards().len(),
        game.sets.len(),
        game.zones.len(),
        game.tag_groups.len(),
        game.card_backs.len(),
        game.scenarios.len(),
        game.tokens.len()
    ));
    let resaved = json::encode_indented(&game.to_json()).expect("finite numbers");
    report.write_resaved(&gamedef_path, Ok(&resaved));
    let verdict = compare_with_original(report, "gamedef.json", &raw, &resaved);
    report.say(format!("  gamedef.json round trip: {verdict}"));

    // Images (existence only; decoding needs SDL3_image in the client).
    let mut missing_images = Vec::new();
    let mut count_by_extension: std::collections::BTreeMap<String, usize> = Default::default();
    let mut unique_backs_missing = 0;
    for card in game.cards() {
        let Some(image_path) = &card.image_path else { continue };
        let resolved = match &card.set_id {
            Some(set_id) => folder.join(set_id).join(image_path),
            None => folder.join(image_path),
        };
        let extension = resolved.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        *count_by_extension.entry(if extension.is_empty() { "(none)".into() } else { extension }).or_default() += 1;
        if !resolved.is_file() {
            missing_images.push(format!("{} -> {}", card.id, resolved.display()));
        }
        if card.card_back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID) && !unique_back_path(&resolved).is_file() {
            unique_backs_missing += 1;
        }
    }
    let by_type: Vec<String> =
        count_by_extension.iter().map(|(extension, count)| format!("{extension} x{count}")).collect();
    report.say(format!(
        "  card images by type: {}",
        if by_type.is_empty() { "none (code-drawn cards)".to_string() } else { by_type.join(", ") }
    ));
    if !missing_images.is_empty() {
        report.problem(format!("{} card image(s) missing on disk, e.g.:", missing_images.len()));
        for missing in missing_images.iter().take(5) {
            report.say(format!("      {missing}"));
        }
    }
    if unique_backs_missing > 0 {
        report.say(format!("  note: {unique_backs_missing} card(s) use a Unique back with no _BACK file"));
    }
    for back in &game.card_backs {
        if let Some(image_path) = &back.image_path
            && !folder.join(image_path).is_file()
        {
            report.problem(format!("card back '{}' image missing: {image_path}", back.name));
        }
    }

    // Built-in decks: _decks/ wins over decks/.
    let built_in_folder = if folder.join("_decks").is_dir() { folder.join("_decks") } else { folder.join("decks") };
    for deck_path in json_files_in(&built_in_folder) {
        check_deck_file(report, &deck_path, Some(&game));
    }

    // Saved games.
    for save_path in json_files_in(&folder.join("savegames")) {
        let raw_save = match read_text(&save_path) {
            Ok(raw) => raw,
            Err(error) => {
                report.problem(format!("save unreadable: {}: {error}", save_path.display()));
                continue;
            }
        };
        let parsed = json::decode(&raw_save)
            .map_err(|error| error.to_string())
            .and_then(|value| SavedGame::from_json(&value).map_err(|error| error.to_string()));
        let label = save_path.file_stem().unwrap().to_string_lossy().to_string();
        match parsed {
            Ok(saved) => {
                let resaved = json::encode_indented(&saved.to_json()).expect("finite numbers");
                report.write_resaved(&save_path, Ok(&resaved));
                let verdict = compare_with_original(report, &format!("save {label}"), &raw_save, &resaved);
                let unknown =
                    saved.state.cards().iter().filter(|card| game.card_by_id(&card.definition_id).is_none()).count();
                report.say(format!(
                    "  save '{label}' ({} players, {} cards, {} widgets, {} log lines): round trip {verdict}",
                    saved.state.players.len(),
                    saved.state.cards().len(),
                    saved.state.widgets().len(),
                    saved.state.log.len()
                ));
                if unknown > 0 {
                    report.problem(format!("save '{label}' references {unknown} card(s) not in the game"));
                }
            }
            Err(error) => {
                report.write_resaved(&save_path, Err(error.clone()));
                report.problem(format!("save '{label}' unreadable: {error}"));
            }
        }
    }
    Some(game)
}

fn check_deck_file(report: &mut Report, deck_path: &Path, game: Option<&GameDefinition>) {
    let label = deck_path.strip_prefix(&report.library_root).unwrap_or(deck_path).display().to_string();
    let raw = match read_text(deck_path) {
        Ok(raw) => raw,
        Err(error) => {
            report.problem(format!("{label}: {error}"));
            return;
        }
    };
    let parsed =
        json::decode(&raw).map_err(|error| error.to_string()).and_then(|value| match DeckConfig::from_json(&value) {
            Err(ModelError::LegacyDeckFormat) => Err("legacy".to_string()),
            other => other.map_err(|error| error.to_string()),
        });
    match parsed {
        Ok(deck) => {
            let resaved = json::encode_compact(&deck.to_json()).expect("finite numbers");
            report.write_resaved(deck_path, Ok(&resaved));
            let verdict = compare_with_original(report, &format!("deck {label}"), &raw, &resaved);
            let mismatch = game.is_some_and(|game| game.id != deck.game_id);
            report.say(format!(
                "  {label}: {} cards in {} subdeck(s), gameId {}{}; round trip {verdict}",
                deck.total_card_count(),
                deck.subdecks.len(),
                deck.game_id,
                if mismatch { " (MISMATCH: not listed for this game)" } else { "" }
            ));
            if let Some(game) = game {
                let unknown = deck
                    .subdecks
                    .iter()
                    .flat_map(|subdeck| &subdeck.entries)
                    .filter(|entry| game.card_by_id(&entry.definition_id).is_none())
                    .count();
                if unknown > 0 {
                    report.problem(format!("{label} references {unknown} unknown card id(s)"));
                }
            }
        }
        Err(reason) if reason == "legacy" => {
            report.write_resaved(deck_path, Err(ModelError::LEGACY_DECK_FORMAT_MESSAGE.to_string()));
            report.say(format!("  {label}: old format (the original also rejects it)"));
        }
        Err(reason) => {
            report.write_resaved(deck_path, Err(reason.clone()));
            report.problem(format!("{label}: {reason}"));
        }
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut library_root = None;
    let mut resaved_output = None;
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if argument == "--resaved-output" {
            resaved_output = remaining.next().map(PathBuf::from);
        } else if library_root.is_none() {
            library_root = Some(PathBuf::from(argument));
        }
    }
    let Some(library_root) = library_root else {
        eprintln!("usage: compat_check <libraryRoot> [--resaved-output <dir>]");
        return ExitCode::from(2);
    };
    let library_root = std::fs::canonicalize(&library_root).unwrap_or(library_root);
    if let Some(output) = &resaved_output {
        std::fs::create_dir_all(output).expect("create output directory");
        let output = std::fs::canonicalize(output).unwrap();
        if output.starts_with(&library_root) {
            eprintln!("refusing to write inside the library: {}", output.display());
            return ExitCode::from(2);
        }
    }

    let started = std::time::Instant::now();
    let mut report = Report { lines: Vec::new(), problem_count: 0, resaved_output, library_root: library_root.clone() };

    let games_root = library_root.join("game_library");
    report.say(format!("== Games in {}", games_root.display()));
    let mut games = Vec::new();
    for folder in sorted_entries(&games_root).into_iter().filter(|path| path.is_dir()) {
        if let Some(game) = check_game_folder(&mut report, &folder) {
            games.push(game);
        }
    }

    let decks_root = library_root.join("deck_library");
    report.say(format!("\n== Decks in {}", decks_root.display()));
    for game_folder in sorted_entries(&decks_root).into_iter().filter(|path| path.is_dir()) {
        let folder_name = game_folder.file_name().unwrap().to_string_lossy().to_string();
        let game = games.iter().find(|game| game.id == folder_name);
        if game.is_none() {
            report.say(format!("\n-- folder '{folder_name}': no loaded game has this id"));
        }
        for deck_path in json_files_in(&game_folder) {
            check_deck_file(&mut report, &deck_path, game);
        }
    }

    report.say(format!(
        "\n== Done in {:.1}s: {} game(s), {} problem(s)",
        started.elapsed().as_secs_f64(),
        games.len(),
        report.problem_count
    ));
    if report.problem_count == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
