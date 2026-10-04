//! Timestamps: RFC 3339 in UTC, the one form the database stores.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Unix milliseconds as `2026-10-04T12:34:56.789Z`.
pub fn rfc3339(ms: u64) -> String {
    let (secs, millis) = (ms / 1000, ms % 1000);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn now_rfc3339() -> String {
    rfc3339(now_ms())
}

/// A timestamp from a source, read as UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// RFC 3339 UTC.
    pub utc: String,
    /// Whether the source said which zone it meant. If not, UTC was assumed.
    pub zone_stated: bool,
    /// `second`, `minute` or `day`.
    pub precision: &'static str,
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Read a timestamp as a source wrote it: Unix seconds or milliseconds, or
/// `YYYY-MM-DD[(T| )HH:MM[:SS[.fff]]][Z|+HH:MM]`. `None` when it is neither — the raw
/// string is kept by the caller either way, so nothing is guessed and nothing is lost.
pub fn parse_source_time(raw: &str) -> Option<Parsed> {
    let s = raw.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.len() >= 9 {
        let n: u64 = s.parse().ok()?;
        let ms = if s.len() >= 12 { n } else { n * 1000 };
        return Some(Parsed {
            utc: rfc3339(ms),
            zone_stated: true,
            precision: "second",
        });
    }
    let num = |t: &str| -> Option<i64> {
        (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok())?
    };
    let (date, rest) = (s.get(..10)?, s.get(10..)?);
    let mut d = date.split('-');
    let (y, m, day) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    if d.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(y, m, day);
    if rest.is_empty() {
        return Some(Parsed {
            utc: rfc3339(u64::try_from(days * 86_400_000).ok()?),
            zone_stated: false,
            precision: "day",
        });
    }
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    // Split off the zone: Z, or a sign after the time.
    let (clock, offset_min, stated) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, 0, true)
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let (c, z) = rest.split_at(i);
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let z = &z[1..];
        let (h, mi) = match z.split_once(':') {
            Some((h, mi)) => (num(h)?, num(mi)?),
            None if z.len() == 4 => (num(&z[..2])?, num(&z[2..])?),
            None => (num(z)?, 0),
        };
        (c, sign * (h * 60 + mi), true)
    } else {
        (rest, 0, false)
    };
    let (hms, frac) = match clock.split_once('.') {
        Some((a, f)) => (a, f),
        None => (clock, ""),
    };
    let mut t = hms.split(':');
    let (h, mi) = (num(t.next()?)?, num(t.next()?)?);
    let (sec, precision) = match t.next() {
        Some(x) => (num(x)?, "second"),
        None => (0, "minute"),
    };
    if t.next().is_some() || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let millis: i64 = if frac.is_empty() {
        0
    } else {
        let f: String = frac.chars().chain("000".chars()).take(3).collect();
        num(&f)?
    };
    let ms = (days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60) * 1000 + millis;
    Some(Parsed {
        utc: rfc3339(u64::try_from(ms).ok()?),
        zone_stated: stated,
        precision,
    })
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn known_instants_format_as_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339(1_791_117_296_789), "2026-10-04T12:34:56.789Z");
    }

    #[test]
    fn source_timestamps_are_read_as_utc_and_say_what_was_assumed() {
        use super::parse_source_time as p;
        let t = p("2026-10-01T03:12:07Z").unwrap();
        assert_eq!(
            (t.utc.as_str(), t.zone_stated, t.precision),
            ("2026-10-01T03:12:07.000Z", true, "second")
        );
        // An offset is applied; a missing zone is assumed to be UTC and flagged.
        assert_eq!(
            p("2026-10-01T05:12:07.5+02:00").unwrap().utc,
            "2026-10-01T03:12:07.500Z"
        );
        assert_eq!(
            p("2026-09-30 22:12:07-0500").unwrap().utc,
            "2026-10-01T03:12:07.000Z"
        );
        let naive = p("2026-10-01 03:12").unwrap();
        assert_eq!(
            (naive.utc.as_str(), naive.zone_stated, naive.precision),
            ("2026-10-01T03:12:00.000Z", false, "minute")
        );
        assert_eq!(p("2026-10-01").unwrap().precision, "day");
        assert_eq!(p("1791117296").unwrap().utc, "2026-10-04T12:34:56.000Z");
        assert_eq!(p("1791117296789").unwrap().utc, "2026-10-04T12:34:56.789Z");
        for bad in ["", "yesterday", "2026-13-01", "2026-10-01T25:00:00Z", "12"] {
            assert!(p(bad).is_none(), "{bad}");
        }
    }
}
