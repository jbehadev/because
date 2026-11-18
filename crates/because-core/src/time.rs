//! The little date handling `because` needs: UTC only, RFC 3339 as Claude
//! Code and git write it. Not worth a dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, as `2026-10-02T16:03:20.117Z`.
pub fn now_rfc3339() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format_ms(d.as_millis() as i64)
}

/// Milliseconds since the epoch → `2026-10-02T16:03:20.117Z`.
pub fn format_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60,
        ms.rem_euclid(1000)
    )
}

/// `2026-10-02T16:03:20.117Z` (or with `+hh:mm`, or no fraction) → milliseconds
/// since the epoch.
pub fn parse_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let digits = &s[start..i];
        frac = format!("{:0<3}", &digits[..digits.len().min(3)]).parse().ok()?;
    }
    let offset_min = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(c @ (b'+' | b'-')) => {
            let o = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
            if *c == b'-' { -o } else { o }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some((days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60) * 1000 + frac)
}

/// `2026-10-02T…` → `Oct 2`, or `Oct 2 2025` when not this year. UTC.
pub fn short_date(s: &str) -> String {
    let Some(ms) = parse_ms(s) else { return String::new() };
    let (y, mo, d) = civil(ms.div_euclid(86_400_000));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (this_year, _, _) = civil(now.div_euclid(86_400));
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = M[(mo - 1).clamp(0, 11) as usize];
    if y == this_year { format!("{m} {d}") } else { format!("{m} {d} {y}") }
}

/// `75_000` → `1m 15s`.
pub fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}m {:02}s", s / 60, s % 60)
}

// Howard Hinnant's civil-from-days and its inverse.
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_known_values() {
        // `date -u -d 2026-10-02T16:03:20Z +%s` prints 1790957000
        assert_eq!(parse_ms("2026-10-02T16:03:20Z"), Some(1_790_957_000_000));
        let ms = parse_ms("2026-09-22T02:18:34.950Z").unwrap();
        assert_eq!(format_ms(ms), "2026-09-22T02:18:34.950Z");
        assert_eq!(parse_ms("2026-09-22T04:18:34.950+02:00"), Some(ms));
        assert_eq!(parse_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(format_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(parse_ms("not a date"), None);
        assert_eq!(fmt_duration(75_000), "1m 15s");
    }
}
