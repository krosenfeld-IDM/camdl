//! Calendar-time conversion — the **boundary translator** between ISO calendar
//! dates and camdl's internal continuous time axis (2026-05-22 proposal).
//!
//! Dates live only at the I/O edge: this module is the *only* place a calendar
//! date becomes (or is recovered from) an `f64` internal time. Below it,
//! everything is `f64` time in units of the model's `time_unit`, measured from
//! `origin`.
//!
//! **Cross-language contract.** The day-number (`rata_die`) algorithm and the
//! `days_per_unit` table MUST match the OCaml compiler
//! (`ocaml/lib/compiler/expander.ml`: `days_of_date` / `parse_date_to_float`)
//! exactly, so a `date()` literal in a model and the same date in a data file
//! convert identically. Both are pinned by the golden table in
//! `ir/golden/caltime.tsv`, checked by a Rust test here and an OCaml test.
//!
//! v1 scope: **dates only** (`YYYY-MM-DD`), naive (no timezone semantics — a
//! trailing zone designator is *discarded*, reducing to the civil date). Times
//! of day (`…THH:MM:SS`) are rejected; they are deferred (proposal F2).

/// Error parsing or converting a calendar instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalError {
    /// Not a `YYYY-MM-DD` date (with an optional discarded zone designator).
    BadFormat(String),
    /// Month not in 1..=12, or day not valid for the month/year.
    OutOfRange(String),
    /// A time-of-day component (`T…` / space + time) — deferred to F2.
    DatetimeUnsupported(String),
    /// `time_unit` is not a recognised calendar unit.
    UnknownUnit(String),
}

impl std::fmt::Display for CalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalError::BadFormat(s) => write!(
                f,
                "expected an ISO date 'YYYY-MM-DD' (optionally with a discarded zone), got '{s}'"
            ),
            CalError::OutOfRange(s) => write!(f, "date out of range: '{s}'"),
            CalError::DatetimeUnsupported(s) => write!(
                f,
                "time-of-day is not supported (dates only in v1): '{s}'"
            ),
            CalError::UnknownUnit(s) => write!(
                f,
                "'{s}' is not a calendar time unit (expected days/weeks/months/years)"
            ),
        }
    }
}

impl std::error::Error for CalError {}

/// Proleptic-Gregorian day number — **identical formula to the OCaml
/// `days_of_date`** (Hatcher/Richards; the `-694025` epoch offset is arbitrary
/// but shared, so absolute day numbers match too; only deltas are load-bearing).
/// Valid for dates CE 1583+.
pub fn rata_die(y: i64, m: i64, d: i64) -> i64 {
    let (yy, mm) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    365 * yy + yy / 4 - yy / 100 + yy / 400 + (153 * (mm + 1)) / 5 + d - 694025
}

/// Canonical duration of one `time_unit` in **days**. Matches the OCaml `D`
/// table. `months`/`years` are *average* lengths (365.2425-day Gregorian year).
pub fn days_per_unit(time_unit: &str) -> Result<f64, CalError> {
    match time_unit {
        "days" => Ok(1.0),
        "weeks" => Ok(7.0),
        "months" => Ok(365.2425 / 12.0),
        "years" => Ok(365.2425),
        other => Err(CalError::UnknownUnit(other.to_string())),
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Parse an ISO calendar date `YYYY-MM-DD`, returning `(year, month, day)`.
///
/// Accepts a trailing **zone designator** (`Z`, `+HH:MM`, `-HH:MM`) and
/// **discards** it (a bare date denotes a civil-calendar day, zone-independent —
/// proposal §6.8). Rejects time-of-day forms (`T…` / space + time) as
/// `DatetimeUnsupported` (v1). Validates month and day (leap-aware).
pub fn parse_iso_date(s: &str) -> Result<(i64, i64, i64), CalError> {
    let s = s.trim();
    // The date portion is the first 10 chars: YYYY-MM-DD.
    if s.len() < 10 {
        return Err(CalError::BadFormat(s.to_string()));
    }
    let (date_part, rest) = s.split_at(10);

    // Classify the remainder: empty or a bare zone designator → discard;
    // a time-of-day (T/space then digits) → datetime, rejected in v1.
    if !rest.is_empty() {
        let is_zone = rest == "Z"
            || rest == "z"
            || ((rest.starts_with('+') || rest.starts_with('-'))
                && rest.len() == 6
                && rest.as_bytes()[3] == b':'
                && rest[1..3].chars().all(|c| c.is_ascii_digit())
                && rest[4..6].chars().all(|c| c.is_ascii_digit()));
        if !is_zone {
            // A `T` or space followed by time-of-day, or any other trailer.
            return Err(CalError::DatetimeUnsupported(s.to_string()));
        }
    }

    let bytes = date_part.as_bytes();
    // Strict YYYY-MM-DD shape.
    let shape_ok = bytes[4] == b'-'
        && bytes[7] == b'-'
        && date_part[0..4].chars().all(|c| c.is_ascii_digit())
        && date_part[5..7].chars().all(|c| c.is_ascii_digit())
        && date_part[8..10].chars().all(|c| c.is_ascii_digit());
    if !shape_ok {
        return Err(CalError::BadFormat(s.to_string()));
    }
    let y: i64 = date_part[0..4].parse().map_err(|_| CalError::BadFormat(s.to_string()))?;
    let m: i64 = date_part[5..7].parse().map_err(|_| CalError::BadFormat(s.to_string()))?;
    let d: i64 = date_part[8..10].parse().map_err(|_| CalError::BadFormat(s.to_string()))?;

    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return Err(CalError::OutOfRange(s.to_string()));
    }
    Ok((y, m, d))
}

/// Convert an ISO date string to internal time, given the `origin` date string
/// and the model's `time_unit`:
/// `t = (rata_die(date) − rata_die(origin)) / days_per_unit(unit)`.
/// May be negative (date before origin); fractional under non-day units.
pub fn date_to_internal(origin: &str, date: &str, time_unit: &str) -> Result<f64, CalError> {
    let (oy, om, od) = parse_iso_date(origin)?;
    let (ty, tm, td) = parse_iso_date(date)?;
    let delta = rata_die(ty, tm, td) - rata_die(oy, om, od);
    Ok(delta as f64 / days_per_unit(time_unit)?)
}

/// Civil date from a rata-die day number (inverse of `rata_die`). Used to render
/// internal times back as dates. Algorithm: Howard Hinnant's `civil_from_days`,
/// shifted by the same `-694025` epoch offset `rata_die` uses.
pub fn date_from_rata_die(rd: i64) -> (i64, i64, i64) {
    // Convert our epoch (rata_die) to days-since-1970 used by the civil algo.
    // rata_die(1970,1,1):
    let z = rd - rata_die(1970, 1, 1) + 719_468; // days since 0000-03-01 era
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// Render an internal time back to an ISO date, given `origin` and `time_unit`
/// (inverse of [`date_to_internal`]). Rounds to the nearest whole day.
pub fn internal_to_date(origin: &str, t: f64, time_unit: &str) -> Result<String, CalError> {
    let (oy, om, od) = parse_iso_date(origin)?;
    let delta_days = (t * days_per_unit(time_unit)?).round() as i64;
    let (y, m, d) = date_from_rata_die(rata_die(oy, om, od) + delta_days);
    Ok(format!("{y:04}-{m:02}-{d:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_and_century_rules() {
        // 2000 is a leap year (divisible by 400); 1900 is not (by 100, not 400).
        assert_eq!(rata_die(2000, 3, 1) - rata_die(2000, 2, 28), 2); // Feb 29 exists
        assert_eq!(rata_die(1900, 3, 1) - rata_die(1900, 2, 28), 1); // no Feb 29
        assert_eq!(rata_die(2020, 3, 1) - rata_die(2020, 2, 28), 2); // 2020 leap
        assert!(parse_iso_date("2020-02-29").is_ok());
        assert!(matches!(parse_iso_date("2021-02-29"), Err(CalError::OutOfRange(_))));
        assert!(matches!(parse_iso_date("1900-02-29"), Err(CalError::OutOfRange(_))));
        assert!(parse_iso_date("2000-02-29").is_ok());
    }

    #[test]
    fn month_boundary_deltas() {
        assert_eq!(rata_die(2020, 2, 1) - rata_die(2020, 1, 1), 31); // Jan→Feb
        assert_eq!(rata_die(2020, 3, 1) - rata_die(2020, 2, 1), 29); // Feb→Mar (leap)
        assert_eq!(rata_die(2021, 1, 1) - rata_die(2020, 12, 1), 31); // Dec→Jan
    }

    #[test]
    fn sign_and_zero() {
        assert_eq!(date_to_internal("2020-02-28", "2020-02-28", "days").unwrap(), 0.0);
        assert_eq!(date_to_internal("2020-02-28", "2020-02-18", "days").unwrap(), -10.0);
        // antisymmetry
        let a = date_to_internal("2020-01-01", "2020-03-01", "days").unwrap();
        let b = date_to_internal("2020-03-01", "2020-01-01", "days").unwrap();
        assert_eq!(a, -b);
    }

    #[test]
    fn per_unit_division() {
        // 14 days under each unit.
        let d14 = || date_to_internal("2020-01-01", "2020-01-15", "days").unwrap();
        assert_eq!(d14(), 14.0); // exact integer f64 under 'days
        assert_eq!(date_to_internal("2020-01-01", "2020-01-15", "weeks").unwrap(), 14.0 / 7.0);
        assert!((date_to_internal("2020-01-01", "2020-01-15", "years").unwrap()
            - 14.0 / 365.2425)
            .abs()
            < 1e-12);
        assert!(matches!(days_per_unit("fortnights"), Err(CalError::UnknownUnit(_))));
    }

    #[test]
    fn round_trip() {
        for date in ["2019-01-01", "2020-02-29", "2020-12-31", "1861-10-01", "2026-05-22"] {
            let t = date_to_internal("2020-02-28", date, "days").unwrap();
            let back = internal_to_date("2020-02-28", t, "days").unwrap();
            assert_eq!(back, date, "round-trip failed for {date}");
        }
        // Negative internal time round-trips to a date before the origin.
        let t = date_to_internal("2020-02-28", "2020-01-21", "days").unwrap();
        assert!(t < 0.0);
        assert_eq!(internal_to_date("2020-02-28", t, "days").unwrap(), "2020-01-21");
    }

    #[test]
    fn grammar_accepts_zone_discards_it() {
        // Trailing zone designators are accepted and reduced to the civil date.
        for s in ["2020-03-15", "2020-03-15Z", "2020-03-15+06:00", "2020-03-15-03:00", "2020-03-15+05:45"] {
            assert_eq!(parse_iso_date(s).unwrap(), (2020, 3, 15), "for {s}");
        }
    }

    #[test]
    fn grammar_rejects() {
        // datetime forms (v1)
        assert!(matches!(parse_iso_date("2020-03-15T12:00"), Err(CalError::DatetimeUnsupported(_))));
        assert!(matches!(parse_iso_date("2020-03-15 12:00"), Err(CalError::DatetimeUnsupported(_))));
        // malformed
        for s in ["2020/03/15", "15-03-2020", "20-03-15", "2020-3-15", "", "2020-03", "abc"] {
            assert!(parse_iso_date(s).is_err(), "should reject '{s}'");
        }
        // out of range
        assert!(matches!(parse_iso_date("2020-13-01"), Err(CalError::OutOfRange(_))));
        assert!(matches!(parse_iso_date("2020-02-30"), Err(CalError::OutOfRange(_))));
        assert!(matches!(parse_iso_date("2020-00-10"), Err(CalError::OutOfRange(_))));
    }

    /// Multi-timezone civil-date alignment: same date, different offsets → same t;
    /// genuinely different dates → consecutive integer t (proposal §9.7).
    #[test]
    fn timezone_independent_civil_dates() {
        let same: Vec<f64> = ["2020-03-15+01:00", "2020-03-15+06:00", "2020-03-15-03:00", "2020-03-15+05:45", "2020-03-15Z"]
            .iter()
            .map(|s| date_to_internal("2020-03-01", s, "days").unwrap())
            .collect();
        assert!(same.iter().all(|&t| t == same[0]), "offsets must collapse to one civil date");
        // distinct civil dates → consecutive integers
        let t15 = date_to_internal("2020-03-01", "2020-03-15", "days").unwrap();
        let t16 = date_to_internal("2020-03-01", "2020-03-16", "days").unwrap();
        let t17 = date_to_internal("2020-03-01", "2020-03-17", "days").unwrap();
        assert_eq!((t15, t16, t17), (14.0, 15.0, 16.0));
    }
}
