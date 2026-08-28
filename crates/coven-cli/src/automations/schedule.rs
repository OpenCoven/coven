//! Schedule math for routine recurrence (coven#816).
//!
//! Given the scoped RRULE vocabulary from `rrule.rs` and a definition
//! timezone, compute the next occurrence instant strictly after a cursor.
//! The planner in `occurrences.rs` walks this function forward to find the
//! latest due slot for misfire-latest semantics.

use chrono::{
    DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc, Weekday,
};

use super::definition::RoutineTimezone;
use super::rrule::{parse_rrule, ParsedRrule, RruleFrequency};

/// The earliest schedule instant strictly after `from`, or `None` when the
/// recurrence vocabulary cannot produce one (unreachable for the supported
/// DAILY/WEEKLY subset, kept for forward compatibility).
pub fn next_due(
    rrule_text: &str,
    timezone: RoutineTimezone,
    from: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    let parsed = parse_rrule(rrule_text)?;
    Ok(next_due_parsed(&parsed, timezone, from))
}

/// Interprets a naive local date+hour in the definition's timezone and
/// returns the UTC instant, skipping wall-clock times that do not exist
/// (DST spring-forward gaps).
fn resolve_local(timezone: RoutineTimezone, date: NaiveDate, hour: u8) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(hour as u32, 0, 0)?;
    match timezone {
        RoutineTimezone::Utc => Some(Utc.from_utc_datetime(&naive)),
        RoutineTimezone::Local => match Local.from_local_datetime(&naive) {
            LocalResult::Single(instant) => Some(instant.with_timezone(&Utc)),
            // DST fall-back repeats the hour: take the earliest (first pass).
            LocalResult::Ambiguous(earliest, _) => Some(earliest.with_timezone(&Utc)),
            LocalResult::None => None,
        },
    }
}

fn weekday_index(weekday: Weekday) -> u32 {
    weekday.num_days_from_monday()
}

fn next_due_parsed(
    parsed: &ParsedRrule,
    timezone: RoutineTimezone,
    from: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    // Walk up to 9 days of candidate dates; the supported subset always
    // yields a candidate inside this window (DAILY=1 day, WEEKLY<=7 days).
    let window_start = match timezone {
        RoutineTimezone::Utc => from.with_timezone(&Utc).date_naive(),
        RoutineTimezone::Local => from.with_timezone(&Local).date_naive(),
    };

    let allowed_days: Vec<u32> = match parsed.frequency {
        RruleFrequency::Daily => (0..7).collect(),
        RruleFrequency::Weekly => {
            let mut days: Vec<u32> = parsed
                .by_day
                .iter()
                .filter_map(|day| match day.as_str() {
                    "MO" => Some(0),
                    "TU" => Some(1),
                    "WE" => Some(2),
                    "TH" => Some(3),
                    "FR" => Some(4),
                    "SA" => Some(5),
                    "SU" => Some(6),
                    _ => None,
                })
                .collect();
            days.sort_unstable();
            days
        }
    };

    for offset in 0..10i64 {
        let date = window_start + Duration::days(offset);
        let is_allowed_day = match parsed.frequency {
            RruleFrequency::Daily => true,
            RruleFrequency::Weekly => allowed_days.contains(&weekday_index(date.weekday())),
        };
        if !is_allowed_day {
            continue;
        }

        for hour in &parsed.by_hour {
            if let Some(instant) = resolve_local(timezone, date, *hour) {
                if instant > from {
                    return Some(instant);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn daily_nine_after_ten_is_next_morning() {
        let next = next_due(
            "FREQ=DAILY;BYHOUR=9",
            RoutineTimezone::Utc,
            utc(2026, 8, 28, 10, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, utc(2026, 8, 29, 9, 0));
    }

    #[test]
    fn daily_nine_before_nine_is_today() {
        let next = next_due(
            "FREQ=DAILY;BYHOUR=9",
            RoutineTimezone::Utc,
            utc(2026, 8, 28, 8, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, utc(2026, 8, 28, 9, 0));
    }

    #[test]
    fn twice_daily_picks_the_next_of_two_hours() {
        let next = next_due(
            "FREQ=DAILY;BYHOUR=9,17",
            RoutineTimezone::Utc,
            utc(2026, 8, 28, 12, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, utc(2026, 8, 28, 17, 0));
    }

    #[test]
    fn weekly_skips_to_the_next_allowed_day() {
        // 2026-08-28 is a Friday. MO,WE,FR -> next is Monday 08-31.
        let next = next_due(
            "FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=8",
            RoutineTimezone::Utc,
            utc(2026, 8, 28, 9, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, utc(2026, 8, 31, 8, 0));
    }

    #[test]
    fn weekly_same_day_later_hour() {
        let next = next_due(
            "FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=8,17",
            RoutineTimezone::Utc,
            utc(2026, 8, 28, 9, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, utc(2026, 8, 28, 17, 0));
    }

    #[test]
    fn default_hour_is_nine() {
        let next = next_due("FREQ=DAILY", RoutineTimezone::Utc, utc(2026, 8, 28, 10, 0))
            .unwrap()
            .unwrap();
        assert_eq!(next, utc(2026, 8, 29, 9, 0));
    }
}
