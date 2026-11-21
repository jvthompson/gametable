//! Saved games and game logs. Port of `src/data/save_files.lua`.
//!
//! Saves live in `<game folder>/savegames/<name>.json` (indented JSON), logs in
//! `<game folder>/gamelogs/<file name>`. A `.gamedef` package can't be written to, so for a
//! packaged game both go under `<library>/.gamedata/<id>/` instead (`writable_game_folder`).

use std::path::Path;

use gametable_core::json;
use gametable_core::models::{LocalDateTime, LogEntry, SavedGame, format_elapsed_time, parse_iso_timestamp};

use super::file_system::{join_path, list_folder, strip_extension};
use super::game_files::writable_game_folder;

fn saves_folder(game_folder_path: &str) -> String {
    join_path(&[&writable_game_folder(game_folder_path), "savegames"])
}

fn save_path(game_folder_path: &str, name: &str) -> String {
    join_path(&[&saves_folder(game_folder_path), &format!("{name}.json")])
}

pub fn save_exists(game_folder_path: &str, name: &str) -> bool {
    Path::new(&save_path(game_folder_path, name)).is_file()
}

/// Writes the save (indented JSON) and returns its path.
pub fn write_save_game(game_folder_path: &str, name: &str, save: &SavedGame) -> Result<String, String> {
    std::fs::create_dir_all(saves_folder(game_folder_path)).map_err(|error| error.to_string())?;
    let path = save_path(game_folder_path, name);
    let text = json::encode_indented(&save.to_json()).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| format!("{path}: {error}"))?;
    Ok(path)
}

pub fn read_save_game(file_path: &str) -> Result<SavedGame, String> {
    let bytes = std::fs::read(file_path).map_err(|error| format!("{file_path}: {error}"))?;
    let raw = json::decode_bytes(&bytes).map_err(|error| error.to_string())?;
    SavedGame::from_json(&raw).map_err(|error| error.to_string())
}

/// A save in the Load Game list.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveGameListing {
    pub display_name: String,
    pub file_path: String,
    pub saved_at: String,
    pub player_count: usize,
}

/// Sort key for a saved-at timestamp: `YYYYMMDDHHMMSS` plus the fraction as written (Dart's
/// fixed-width form sorts correctly as text); empty when it can't be parsed.
fn sort_key(iso: &str) -> String {
    let Some(time) = parse_iso_timestamp(iso) else { return String::new() };
    let mut key =
        format!("{:04}{:02}{:02}{:02}{:02}{:02}", time.year, time.month, time.day, time.hour, time.minute, time.second);
    // Lua: iso:match("%d%d:%d%d:%d%d(%.%d+)") -- the first "HH:MM:SS" followed by ".digits".
    let bytes = iso.as_bytes();
    for start in 0..bytes.len().saturating_sub(7) {
        let window = &bytes[start..start + 8];
        let is_clock = window
            .iter()
            .enumerate()
            .all(|(index, byte)| if index == 2 || index == 5 { *byte == b':' } else { byte.is_ascii_digit() });
        if !is_clock {
            continue;
        }
        let rest = &iso[start + 8..];
        let fraction_digits =
            rest.strip_prefix('.').map_or(0, |after| after.bytes().take_while(u8::is_ascii_digit).count());
        if fraction_digits > 0 {
            key.push_str(&rest[..fraction_digits + 1]);
            break;
        }
    }
    key
}

/// Newest first. Unreadable or foreign JSON files are skipped.
pub fn list_save_games(game_folder_path: &str) -> Vec<SaveGameListing> {
    let folder = saves_folder(game_folder_path);
    let mut listings: Vec<(String, SaveGameListing)> = Vec::new();
    for item in list_folder(Path::new(&folder)).unwrap_or_default() {
        if item.is_folder || !item.name.to_lowercase().ends_with(".json") {
            continue;
        }
        let file_path = join_path(&[&folder, &item.name]);
        if let Ok(save) = read_save_game(&file_path) {
            listings.push((
                sort_key(&save.saved_at),
                SaveGameListing {
                    display_name: strip_extension(&item.name).to_string(),
                    file_path,
                    player_count: save.state.players.len(),
                    saved_at: save.saved_at,
                },
            ));
        }
    }
    listings.sort_by(|(first_key, _), (second_key, _)| second_key.cmp(first_key));
    listings.into_iter().map(|(_, listing)| listing).collect()
}

/// `"YYYY-MM-DD HH:MM"` for list rows; the text itself when it can't be parsed.
pub fn format_saved_at(iso: &str) -> String {
    match parse_iso_timestamp(iso) {
        Some(time) => {
            format!("{:04}-{:02}-{:02} {:02}:{:02}", time.year, time.month, time.day, time.hour, time.minute)
        }
        None => iso.to_string(),
    }
}

/// The current local date and time (SDL's clock and time zone).
pub fn local_now() -> LocalDateTime {
    use sdl3::sys::time::{SDL_DateTime, SDL_GetCurrentTime, SDL_TimeToDateTime};
    let mut ticks = 0;
    let mut date_time = SDL_DateTime::default();
    // SAFETY: both pointers refer to valid local variables for the duration of the calls.
    let converted = unsafe { SDL_GetCurrentTime(&mut ticks) && SDL_TimeToDateTime(ticks, &mut date_time, true) };
    if !converted {
        return LocalDateTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0, millisecond: 0 };
    }
    LocalDateTime {
        year: date_time.year,
        month: date_time.month as u32,
        day: date_time.day as u32,
        hour: date_time.hour as u32,
        minute: date_time.minute as u32,
        second: date_time.second as u32,
        millisecond: (date_time.nanosecond / 1_000_000) as u32,
    }
}

/// The name the in-game Save Log button gives a log: `gamelog_YYYY-MM-DD_HHMMSS.txt`.
pub fn log_file_name(time: &LocalDateTime) -> String {
    format!(
        "gamelog_{:04}-{:02}-{:02}_{:02}{:02}{:02}.txt",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

/// One `[HH:MM:SS] message` line per entry.
pub fn format_log(entries: &[LogEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("[{}] {}", format_elapsed_time(entry.elapsed_milliseconds), entry.message))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn write_log(game_folder_path: &str, file_name: &str, contents: &str) -> Result<String, String> {
    let folder = join_path(&[&writable_game_folder(game_folder_path), "gamelogs"]);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let path = join_path(&[&folder, file_name]);
    std::fs::write(&path, contents).map_err(|error| format!("{path}: {error}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::{PlayerInfo, PlayerRole, TableState};

    #[test]
    fn sort_keys_and_formatting() {
        assert_eq!(sort_key("2026-01-02T03:04:05.678"), "20260102030405.678");
        assert_eq!(sort_key("2026-01-02T03:04:05"), "20260102030405");
        assert_eq!(sort_key("garbage"), "");
        assert_eq!(format_saved_at("2026-01-02T03:04:05.678"), "2026-01-02 03:04");
        assert_eq!(format_saved_at("garbage"), "garbage");
        let time = LocalDateTime { year: 2026, month: 11, day: 6, hour: 9, minute: 5, second: 3, millisecond: 0 };
        assert_eq!(log_file_name(&time), "gamelog_2026-11-06_090503.txt");
        let entries = [LogEntry::new(3_723_000, "Ann drew."), LogEntry::new(0, "Start")];
        assert_eq!(format_log(&entries), "[01:02:03] Ann drew.\n[00:00:00] Start");
    }

    #[test]
    fn saves_list_newest_first() {
        let root = std::env::temp_dir().join(format!("gametable-saves-{}", gametable_core::uuid::new_v4_uuid()));
        let folder = root.to_string_lossy().into_owned();
        let mut state = TableState::new("g");
        state.players.push(PlayerInfo::new("seat1", "Ann", PlayerRole::Host, 0xFF00_0000));
        for (name, saved_at) in [("old", "2025-01-01T10:00:00.000"), ("new", "2026-01-01T10:00:00.500")] {
            let save = SavedGame { saved_at: saved_at.to_string(), state: state.clone() };
            write_save_game(&folder, name, &save).unwrap();
        }
        std::fs::write(root.join("savegames").join("foreign.json"), "{}").unwrap();
        assert!(save_exists(&folder, "old"));
        let listings = list_save_games(&folder);
        let names: Vec<_> = listings.iter().map(|listing| listing.display_name.as_str()).collect();
        assert_eq!(names, ["new", "old"]);
        assert_eq!(listings[0].player_count, 1);
        assert_eq!(read_save_game(&listings[1].file_path).unwrap().saved_at, "2025-01-01T10:00:00.000");
        let log_path = write_log(&folder, "gamelog_x.txt", "hello").unwrap();
        assert_eq!(std::fs::read_to_string(log_path).unwrap(), "hello");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_time_is_plausible() {
        let now = local_now();
        assert!(now.year >= 2024 && (1..=12).contains(&now.month));
    }
}
