//! Turning an EXIF timestamp into seconds since the Unix epoch.
//!
//! EXIF writes wall-clock time with no time zone: `2026:08:15 23:41:02`. We
//! interpret it as UTC. That is not the true instant, but every frame in a
//! session is off by the same constant, so ordering and the intervals between
//! frames — which is all stacking needs — come out exactly right.
//!
//! Avoiding a date-time dependency here keeps the plugin's dependency tree to
//! rawler alone.

/// Parses `YYYY:MM:DD HH:MM:SS`, the only form EXIF defines.
/// Returns `None` for anything malformed rather than guessing.
pub fn parse_exif(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    // Cameras are inconsistent about the date separator; the rest is fixed.
    let date_separator = |b: u8| b == b':' || b == b'-';
    if !date_separator(bytes[4])
        || !date_separator(bytes[7])
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }

    // `get` rather than direct slicing. The separator checks above prove the
    // *start* of each field is a character boundary, but nothing proves the end
    // of the seconds field is: a corrupted byte that arrives as U+FFFD occupies
    // three bytes, and slicing through it would panic in a function documented
    // to return None for anything malformed.
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    // A leap second reads as 60, and an unset camera clock as all zeroes.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days between the civil date and 1970-01-01, by Howard Hinnant's algorithm.
/// Exact for the whole proleptic Gregorian calendar, with no lookup tables.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    // Shift the year so that March starts it, which puts the leap day last.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let shifted_month = ((month + 9) % 12) as i64; // March = 0
    let day_of_year = (153 * shifted_month + 2) / 5 + day as i64 - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_epoch_and_a_known_instant() {
        assert_eq!(parse_exif("1970:01:01 00:00:00"), Some(0));
        assert_eq!(parse_exif("2000:01:01 00:00:00"), Some(946_684_800));
        assert_eq!(parse_exif("2026:08:27 12:34:56"), Some(1_787_834_096));
    }

    #[test]
    fn handles_leap_days_and_the_century_rule() {
        // 2000 is a leap year, 1900 is not.
        assert_eq!(parse_exif("2000:02:29 00:00:00"), Some(951_782_400));
        assert_eq!(parse_exif("1900:02:29 00:00:00"), None);
        assert_eq!(parse_exif("2024:02:29 00:00:00"), Some(1_709_164_800));
    }

    #[test]
    fn a_night_of_exposures_stays_ordered_and_evenly_spaced() {
        // What the parser is actually for: subframe intervals, across midnight.
        let before = parse_exif("2026:08:27 23:59:30").unwrap();
        let after = parse_exif("2026:08:28 00:00:30").unwrap();
        assert_eq!(after - before, 60);
    }

    #[test]
    fn accepts_the_dash_separator_some_cameras_write() {
        assert_eq!(parse_exif("2026-08-27 12:34:56"), parse_exif("2026:08:27 12:34:56"));
    }

    #[test]
    fn rejects_malformed_and_impossible_stamps() {
        assert_eq!(parse_exif(""), None);
        assert_eq!(parse_exif("2026:08:27"), None);
        assert_eq!(parse_exif("not a timestamp at all"), None);
        assert_eq!(parse_exif("2026:13:01 00:00:00"), None);
        assert_eq!(parse_exif("2026:08:32 00:00:00"), None);
        assert_eq!(parse_exif("2026:04:31 00:00:00"), None);
        assert_eq!(parse_exif("2026:08:27 24:00:00"), None);
        assert_eq!(parse_exif("2026:08:27 12:60:00"), None);
        // An unset camera clock, which must not read as year zero.
        assert_eq!(parse_exif("0000:00:00 00:00:00"), None);
    }

    #[test]
    fn a_corrupted_byte_returns_none_rather_than_panicking() {
        // A replacement character occupies three bytes, so the seconds field
        // would end mid-character. This function promises None for anything
        // malformed, and a panic is not None.
        assert_eq!(parse_exif("2026:08:27 12:34:\u{fffd}"), None);
        assert_eq!(parse_exif("2026:08:27 12:34:5\u{fffd}"), None);
        assert_eq!(parse_exif("\u{fffd}026:08:27 12:34:56"), None);
    }

    #[test]
    fn tolerates_surrounding_whitespace_and_trailing_subseconds() {
        assert_eq!(parse_exif("  2026:08:27 12:34:56  "), parse_exif("2026:08:27 12:34:56"));
        assert_eq!(parse_exif("2026:08:27 12:34:56.789"), parse_exif("2026:08:27 12:34:56"));
    }
}
