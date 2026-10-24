//! Renders a millisecond timestamp as `dd-Mon-yyyy hh:mm` (or
//! `dd/mm/yy hh:mm` for language id 3) from a Gregorian calendar, in GMT.
pub(crate) const MONTHS: [[&str; 12]; 7] = [
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
    [
        "jan", "fév", "mars", "avr", "mai", "juin", "juil", "août", "sept", "oct", "nov", "déc",
    ],
    [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ],
    [
        "jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec",
    ],
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ],
];

/// Proleptic Gregorian civil date from days since 1970-01-01 (any sign).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The timestamp in GMT for a language id (id 3 uses the short form).
pub fn format_datetime_utc(millis: i64, language: i32) -> Option<String> {
    let days = millis.div_euclid(86_400_000);
    let rem = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = (rem / 3_600_000) as u32;
    let minute = (rem % 3_600_000 / 60_000) as u32;
    let two = |v: u32| format!("{}{}", v / 10, v % 10);
    if language == 3 {
        let yy = (year.rem_euclid(100)) as u32;
        return Some(format!(
            "{}/{}/{} {}:{}",
            two(day),
            two(month),
            two(yy),
            two(hour),
            two(minute)
        ));
    }
    let months = MONTHS.get(usize::try_from(language).ok()?)?;
    Some(format!(
        "{}-{}-{} {}:{}",
        two(day),
        months[month as usize - 1],
        year,
        two(hour),
        two(minute)
    ))
}

/// The time in the process local Gregorian calendar.
pub fn format_time_local(millis: i64) -> Option<String> {
    let seconds: libc::time_t = millis.div_euclid(1000);
    let mut calendar = std::mem::MaybeUninit::<libc::tm>::uninit();
    // The reentrant platform API writes the caller-owned tm; no shared C
    // calendar buffer or process timezone mutation is used.
    #[cfg(unix)]
    let ok = unsafe { !libc::localtime_r(&seconds, calendar.as_mut_ptr()).is_null() };
    #[cfg(windows)]
    let ok = unsafe { libc::localtime_s(calendar.as_mut_ptr(), &seconds) == 0 };
    #[cfg(not(any(unix, windows)))]
    let ok = false;
    if !ok {
        return None;
    }
    let c = unsafe { calendar.assume_init() };
    Some(format!("{:02}:{:02}:{:02}", c.tm_hour, c.tm_min, c.tm_sec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_leap_dates_format_like_the_original() {
        assert_eq!(format_datetime_utc(0, 0).unwrap(), "01-Jan-1970 00:00");
        // 2024-02-29 13:07 UTC
        let millis = 19_782 * 86_400_000 + (13 * 3600 + 7 * 60) * 1000;
        assert_eq!(format_datetime_utc(millis, 0).unwrap(), "29-Feb-2024 13:07");
        assert_eq!(format_datetime_utc(millis, 3).unwrap(), "29/02/24 13:07");
        assert_eq!(
            format_datetime_utc(-60_000, 1).unwrap(),
            "31-Dez-1969 23:59"
        );
        assert!(format_datetime_utc(0, 7).is_none());
    }
}
