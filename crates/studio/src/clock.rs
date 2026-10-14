//! Wall-clock timestamps in the form the library stores them (`2026-10-14T12:00:00.000Z`).

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, UTC, as JavaScript's `toISOString()` writes it.
pub fn now_iso() -> String {
    iso(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
}

/// Milliseconds since the Unix epoch as an ISO 8601 UTC timestamp.
pub fn iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil(days);
    let (h, mi, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

/// Parses `YYYY-MM-DDTHH:MM[:SS[.mmm]][Z]` (UTC) to Unix milliseconds.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T').unwrap_or((s, "00:00"));
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, dd) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next().unwrap_or("0").parse().ok()?;
    let sec: f64 = t.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&dd) || !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0.0..61.0).contains(&sec) {
        return None;
    }
    Some(days_from_civil(y, m, dd) * 86_400_000 + h * 3_600_000 + mi * 60_000 + (sec * 1000.0).round() as i64)
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// Howard Hinnant's civil-from-days and its inverse.
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A local wall-clock time typed as `YYYY-MM-DD HH:MM` (or with a `T`) as a UTC ISO timestamp.
pub fn local_to_utc_iso(s: &str) -> Option<String> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let s = s.trim().replace('T', " ");
    let naive = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M").or_else(|_| NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S")).ok()?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(iso(local.timestamp_millis()))
}

/// Now plus `minutes`, as local `YYYY-MM-DD HH:MM` (the schedule field's default).
pub fn local_in(minutes: i64) -> String {
    (chrono::Local::now() + chrono::Duration::minutes(minutes)).format("%Y-%m-%d %H:%M").to_string()
}
