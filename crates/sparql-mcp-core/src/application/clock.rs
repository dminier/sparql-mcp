//! Time as whole seconds, and as the `xsd:dateTime` text the ledger stores.
//!
//! Only UTC, only second precision (`2026-11-18T09:30:00Z`): that is all the
//! job ledger needs, and it avoids a date-time dependency. Everything that
//! decides "how long ago" takes the current time as a parameter, so the
//! expiry logic can be tested with any clock.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// `xsd:dateTime` text for `secs` since the epoch: `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso(secs: u64) -> String {
    let (y, m, d) = civil_from_days(secs / 86_400);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

pub fn iso_now() -> String {
    iso(now_secs())
}

/// Seconds since the epoch for `YYYY-MM-DDTHH:MM:SSZ`, or `None` for anything
/// else (other offsets, fractions of a second, malformed text).
pub fn parse_iso(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let n = |range: std::ops::Range<usize>| text.get(range)?.parse::<u64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, d)? * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// (year, month, day) for a count of days since 1970-01-01.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 for a date; `None` before the epoch.
fn days_from_civil(y: u64, m: u64, d: u64) -> Option<u64> {
    let y = if m <= 2 { y.checked_sub(1)? } else { y };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe).checked_sub(719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants_round_trip() {
        for (secs, text) in [
            (0, "1970-01-01T00:00:00Z"),
            (86_399, "1970-01-01T23:59:59Z"),
            (951_782_400, "2000-02-29T00:00:00Z"), // a leap day
            (1_700_000_000, "2023-11-14T22:13:20Z"),
            (1_794_000_000, "2026-11-06T21:20:00Z"),
        ] {
            assert_eq!(iso(secs), text);
            assert_eq!(parse_iso(text), Some(secs), "{text}");
        }
    }

    #[test]
    fn parsing_rejects_what_is_not_second_precision_utc() {
        for bad in [
            "",
            "2026-11-18",
            "2026-11-18T09:30:00",
            "2026-11-18T09:30:00+01:00",
            "2026-11-18T09:30:00.5Z",
            "2026-13-01T00:00:00Z",
            "2026-11-32T00:00:00Z",
            "2026-11-18T24:00:00Z",
            "not a date at all!!!",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(parse_iso(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn every_second_of_a_day_boundary_round_trips() {
        let start = 1_700_000_000 - 1_700_000_000 % 86_400;
        for s in (start - 5..start + 5).chain([start + 86_395, start + 86_400]) {
            assert_eq!(parse_iso(&iso(s)), Some(s));
        }
    }
}
