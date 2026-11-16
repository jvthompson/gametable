//! Time formatting used by the game log and saved games.

/// `HH:MM:SS` for a game-log entry, e.g. `3723000` ms -> `"01:02:03"`.
pub fn format_elapsed_time(elapsed_milliseconds: i64) -> String {
    let total_seconds = elapsed_milliseconds.div_euclid(1000);
    format!(
        "{:02}:{:02}:{:02}",
        total_seconds.div_euclid(3600),
        total_seconds.rem_euclid(3600).div_euclid(60),
        total_seconds.rem_euclid(60)
    )
}

/// A calendar date and time of day without a time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
}

/// `"YYYY-MM-DDTHH:MM:SS.mmm"`, like Dart's `DateTime.now().toIso8601String()` for local time.
/// The caller supplies the local time (the client reads it from SDL).
pub fn format_iso_local_timestamp(time: &LocalDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.millisecond
    )
}

/// Parses `"YYYY-MM-DDTHH:MM:SS[.fff][Z|+hh:mm]"` (a space may replace `T`; seconds and the time
/// part are optional). Fractions and zones are ignored, as in Lua's `parseIso`.
pub fn parse_iso_timestamp(text: &str) -> Option<LocalDateTime> {
    let bytes = text.as_bytes();
    let digits_at = |start: usize, count: usize| -> Option<u32> {
        let slice = bytes.get(start..start + count)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    let year = digits_at(0, 4)? as i32;
    let has_date_separators = bytes.get(4) == Some(&b'-') && bytes.get(7) == Some(&b'-');
    if !has_date_separators {
        return None;
    }
    let month = digits_at(5, 2)?;
    let day = digits_at(8, 2)?;
    let date_only = LocalDateTime { year, month, day, hour: 0, minute: 0, second: 0, millisecond: 0 };

    let has_time_separator = matches!(bytes.get(10), Some(b'T' | b' '));
    let (Some(hour), Some(minute)) = (digits_at(11, 2), digits_at(14, 2)) else {
        return Some(date_only);
    };
    if !has_time_separator || bytes.get(13) != Some(&b':') {
        return Some(date_only);
    }
    // Lua: ":?(%d*)" -- an optional colon, then any run of digits (possibly empty -> 0).
    let mut seconds_start = 16;
    if bytes.get(16) == Some(&b':') {
        seconds_start = 17;
    }
    let seconds_digit_count = bytes[seconds_start.min(bytes.len())..].iter().take_while(|b| b.is_ascii_digit()).count();
    let second = if seconds_digit_count == 0 {
        0
    } else {
        text[seconds_start..seconds_start + seconds_digit_count].parse().unwrap_or(0)
    };
    Some(LocalDateTime { hour, minute, second, ..date_only })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(format_elapsed_time(3_723_000), "01:02:03");
        assert_eq!(format_elapsed_time(999), "00:00:00");
    }

    #[test]
    fn formats_and_parses_iso_timestamps() {
        let time = LocalDateTime { year: 2026, month: 1, day: 2, hour: 3, minute: 4, second: 5, millisecond: 678 };
        assert_eq!(format_iso_local_timestamp(&time), "2026-01-02T03:04:05.678");
        assert_eq!(parse_iso_timestamp("2026-01-02T03:04:05.678"), Some(LocalDateTime { millisecond: 0, ..time }));
        assert_eq!(parse_iso_timestamp("2026-01-02").map(|t| t.hour), Some(0));
        assert_eq!(parse_iso_timestamp("2026-01-02 03:04").map(|t| (t.hour, t.second)), Some((3, 0)));
        assert_eq!(parse_iso_timestamp("nonsense"), None);
    }
}
