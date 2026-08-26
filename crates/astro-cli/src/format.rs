//! Turning frame descriptions into something readable at a terminal.

/// Formats a shutter time the way a photographer reads it: fractions below a
/// second, plain seconds above.
pub fn exposure(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "-".to_owned();
    }
    if seconds >= 1.0 {
        return format!("{}s", trim_zeros(seconds, 3));
    }
    let denominator = (1.0 / seconds).round();
    if denominator.is_finite() && denominator >= 1.0 {
        format!("1/{denominator:.0}s")
    } else {
        format!("{seconds}s")
    }
}

/// Prints at most `decimals` places, without trailing zeroes.
pub fn trim_zeros(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if !text.contains('.') {
        return text;
    }
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Renders seconds-since-the-epoch as `YYYY-MM-DD HH:MM:SS`.
///
/// EXIF timestamps carry no time zone, and the Canon plugin reads them as UTC,
/// so this prints them back the same way. Within one imaging session that makes
/// the displayed times match what the camera showed.
pub fn timestamp(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let time_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (time_of_day / 3600, (time_of_day % 3600) / 60, time_of_day % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Inverse of the days-from-civil algorithm used by the Canon plugin.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let shifted_month = (5 * day_of_year + 2) / 153; // [0, 11], March = 0
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutter_times_read_the_way_a_photographer_writes_them() {
        assert_eq!(exposure(120.0), "120s");
        assert_eq!(exposure(1.0), "1s");
        assert_eq!(exposure(2.5), "2.5s");
        assert_eq!(exposure(1.0 / 250.0), "1/250s");
        assert_eq!(exposure(f64::NAN), "-");
        assert_eq!(exposure(0.0), "-");
    }

    #[test]
    fn timestamps_round_trip_against_the_plugin_parser() {
        // The same instants the Canon plugin's parser is tested against.
        assert_eq!(timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(timestamp(946_684_800), "2000-01-01 00:00:00");
        assert_eq!(timestamp(1_787_834_096), "2026-08-27 12:34:56");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn timestamps_before_the_epoch_do_not_wrap() {
        assert_eq!(timestamp(-1), "1969-12-31 23:59:59");
    }
}
