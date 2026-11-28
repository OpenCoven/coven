//! The one place stored and wire RFC 3339 timestamps are printed (#1798).
//!
//! Timestamps are persisted and compared as strings, and SQLite reads them
//! back with `strftime` and `julianday`, so every writer must keep producing
//! the same bytes. Each method here delegates to exactly the chrono call it
//! replaced. The golden tests below pin those bytes, so replacing the backend
//! with `jiff` (slice 3 of #1798) changes this file only and the tests prove
//! the output is unchanged.
//!
//! Choose by the shape already stored, never by taste:
//!
//! | Method | Shape |
//! |---|---|
//! | [`Rfc3339::millis_z`] | `2026-11-18T07:00:00.000Z` |
//! | [`Rfc3339::nanos_z`] | `2026-11-18T07:00:00.000000000Z` |
//! | [`Rfc3339::secs_z`] | `2026-11-18T07:00:00Z` |
//! | [`Rfc3339::auto_z`] | 0, 3, 6 or 9 fractional digits, `Z` |
//! | [`Rfc3339::rfc3339_offset`] | like `auto_z`, but UTC prints `+00:00` |
//!
//! `rfc3339_offset` exists because chrono's `to_rfc3339()` prints UTC as
//! `+00:00`, not `Z`. Those writers must not be "normalized" to `Z`: that
//! would change stored values and digests.

use chrono::{DateTime, SecondsFormat, TimeZone};
use std::fmt::Display;

/// Byte-stable RFC 3339 printers for every stored timestamp shape.
pub(crate) trait Rfc3339 {
    /// Milliseconds, `Z` for UTC.
    fn millis_z(&self) -> String;
    /// Nanoseconds, `Z` for UTC.
    fn nanos_z(&self) -> String;
    /// Whole seconds, `Z` for UTC.
    fn secs_z(&self) -> String;
    /// The shortest of 0, 3, 6 or 9 fractional digits, `Z` for UTC.
    fn auto_z(&self) -> String;
    /// The shortest of 0, 3, 6 or 9 fractional digits; UTC prints `+00:00`.
    fn rfc3339_offset(&self) -> String;
}

impl<Tz: TimeZone> Rfc3339 for DateTime<Tz>
where
    Tz::Offset: Display,
{
    fn millis_z(&self) -> String {
        self.to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    fn nanos_z(&self) -> String {
        self.to_rfc3339_opts(SecondsFormat::Nanos, true)
    }

    fn secs_z(&self) -> String {
        self.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn auto_z(&self) -> String {
        self.to_rfc3339_opts(SecondsFormat::AutoSi, true)
    }

    fn rfc3339_offset(&self) -> String {
        self.to_rfc3339()
    }
}

#[cfg(test)]
mod tests {
    use super::Rfc3339;
    use chrono::{DateTime, FixedOffset, TimeZone, Utc};

    fn at(nanos: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 7, 0, 0)
            .unwrap()
            .checked_add_signed(chrono::Duration::nanoseconds(i64::from(nanos)))
            .unwrap()
    }

    /// (fractional nanoseconds, millis_z, nanos_z, secs_z, auto_z, rfc3339_offset)
    const GOLDEN: &[(u32, &str, &str, &str, &str, &str)] = &[
        (
            0,
            "2026-11-18T07:00:00.000Z",
            "2026-11-18T07:00:00.000000000Z",
            "2026-11-18T07:00:00Z",
            "2026-11-18T07:00:00Z",
            "2026-11-18T07:00:00+00:00",
        ),
        (
            1_000_000,
            "2026-11-18T07:00:00.001Z",
            "2026-11-18T07:00:00.001000000Z",
            "2026-11-18T07:00:00Z",
            "2026-11-18T07:00:00.001Z",
            "2026-11-18T07:00:00.001+00:00",
        ),
        (
            123_456_000,
            "2026-11-18T07:00:00.123Z",
            "2026-11-18T07:00:00.123456000Z",
            "2026-11-18T07:00:00Z",
            "2026-11-18T07:00:00.123456Z",
            "2026-11-18T07:00:00.123456+00:00",
        ),
        (
            // Printing truncates toward zero; it never rounds up a digit.
            999_999_999,
            "2026-11-18T07:00:00.999Z",
            "2026-11-18T07:00:00.999999999Z",
            "2026-11-18T07:00:00Z",
            "2026-11-18T07:00:00.999999999Z",
            "2026-11-18T07:00:00.999999999+00:00",
        ),
        (
            1,
            "2026-11-18T07:00:00.000Z",
            "2026-11-18T07:00:00.000000001Z",
            "2026-11-18T07:00:00Z",
            "2026-11-18T07:00:00.000000001Z",
            "2026-11-18T07:00:00.000000001+00:00",
        ),
    ];

    #[test]
    fn every_stored_utc_shape_is_byte_stable() {
        for &(nanos, millis, nanos_z, secs, auto, offset) in GOLDEN {
            let time = at(nanos);
            assert_eq!(time.millis_z(), millis, "millis_z at {nanos} ns");
            assert_eq!(time.nanos_z(), nanos_z, "nanos_z at {nanos} ns");
            assert_eq!(time.secs_z(), secs, "secs_z at {nanos} ns");
            assert_eq!(time.auto_z(), auto, "auto_z at {nanos} ns");
            assert_eq!(
                time.rfc3339_offset(),
                offset,
                "rfc3339_offset at {nanos} ns"
            );
        }
    }

    #[test]
    fn only_rfc3339_offset_prints_utc_as_plus_zero() {
        let time = at(0);
        for printed in [
            time.millis_z(),
            time.nanos_z(),
            time.secs_z(),
            time.auto_z(),
        ] {
            assert!(printed.ends_with('Z'), "{printed}");
        }
        assert!(time.rfc3339_offset().ends_with("+00:00"));
    }

    #[test]
    fn non_utc_offsets_print_the_offset_in_every_shape() {
        let berlin = FixedOffset::east_opt(3600).unwrap();
        let time = at(123_000_000).with_timezone(&berlin);
        assert_eq!(time.millis_z(), "2026-11-18T08:00:00.123+01:00");
        assert_eq!(time.nanos_z(), "2026-11-18T08:00:00.123000000+01:00");
        assert_eq!(time.secs_z(), "2026-11-18T08:00:00+01:00");
        assert_eq!(time.auto_z(), "2026-11-18T08:00:00.123+01:00");
        assert_eq!(time.rfc3339_offset(), "2026-11-18T08:00:00.123+01:00");
    }

    #[test]
    fn every_shape_parses_back_to_the_same_instant_at_its_precision() {
        let time = at(123_456_789);
        let parse = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        assert_eq!(parse(&time.nanos_z()), time);
        assert_eq!(parse(&time.auto_z()), time);
        assert_eq!(parse(&time.rfc3339_offset()), time);
        assert_eq!(parse(&time.millis_z()), at(123_000_000));
        assert_eq!(parse(&time.secs_z()), at(0));
    }
}
