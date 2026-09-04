//! Schedule math for routine recurrence (coven#816).
//!
//! Given the scoped RRULE vocabulary from `rrule.rs` and a definition
//! timezone, compute the next occurrence instant strictly after a cursor.
//! The planner in `occurrences.rs` walks this function forward to find the
//! latest due slot for misfire-latest semantics.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc, Weekday};

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
    if timezone == RoutineTimezone::Local {
        return Err(
            "timezone `local` must be resolved to an exact IANA timezone before scheduling"
                .to_string(),
        );
    }
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
        RoutineTimezone::Iana(timezone) => match timezone.from_local_datetime(&naive) {
            LocalResult::Single(instant) => Some(instant.with_timezone(&Utc)),
            // DST fall-back repeats the hour: take the earliest (first pass).
            LocalResult::Ambiguous(earliest, _) => Some(earliest.with_timezone(&Utc)),
            LocalResult::None => None,
        },
        RoutineTimezone::Local => None,
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
    next_due_parsed_with(parsed, timezone, from, resolve_local)
}

fn next_due_parsed_with(
    parsed: &ParsedRrule,
    timezone: RoutineTimezone,
    from: DateTime<Utc>,
    mut resolve: impl FnMut(RoutineTimezone, NaiveDate, u8) -> Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    // A weekly local-time slot can disappear during a clock transition.
    // Search through the following week's slot so the maximum gap is 14 days.
    let window_start = match timezone {
        RoutineTimezone::Utc | RoutineTimezone::Local => from.date_naive(),
        RoutineTimezone::Iana(timezone) => {
            timezone.from_utc_datetime(&from.naive_utc()).date_naive()
        }
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

    for offset in 0..15i64 {
        let date = window_start + Duration::days(offset);
        let is_allowed_day = match parsed.frequency {
            RruleFrequency::Daily => true,
            RruleFrequency::Weekly => allowed_days.contains(&weekday_index(date.weekday())),
        };
        if !is_allowed_day {
            continue;
        }

        for hour in &parsed.by_hour {
            if let Some(instant) = resolve(timezone, date, *hour) {
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
    use serde_json::json;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn timezone(value: &str) -> RoutineTimezone {
        let definition = super::super::definition::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "timezone-test",
            "name": "Timezone test",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": value,
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Do the thing."
        }))
        .unwrap();
        definition.timezone
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

    #[test]
    fn weekly_search_crosses_a_skipped_local_slot() {
        let parsed = parse_rrule("FREQ=WEEKLY;BYDAY=SU;BYHOUR=2").unwrap();
        let skipped_date = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let from = utc(2026, 3, 1, 12, 0);

        let next = next_due_parsed_with(
            &parsed,
            RoutineTimezone::Local,
            from,
            |_timezone, date, hour| {
                if date == skipped_date {
                    None
                } else {
                    date.and_hms_opt(hour as u32, 0, 0)
                        .map(|naive| Utc.from_utc_datetime(&naive))
                }
            },
        )
        .expect("search should continue through the skipped weekly slot");

        assert_eq!(next, utc(2026, 3, 15, 2, 0));
    }

    #[test]
    fn spring_forward_nonexistent_wall_time_is_skipped() {
        let next = next_due(
            "FREQ=DAILY;BYHOUR=2",
            timezone("America/New_York"),
            utc(2026, 3, 7, 8, 0),
        )
        .unwrap()
        .unwrap();

        assert_eq!(next, utc(2026, 3, 9, 6, 0));
    }

    #[test]
    fn fall_back_ambiguous_wall_time_uses_the_first_occurrence() {
        let next = next_due(
            "FREQ=DAILY;BYHOUR=1",
            timezone("America/New_York"),
            utc(2026, 11, 1, 4, 0),
        )
        .unwrap()
        .unwrap();

        assert_eq!(next, utc(2026, 11, 1, 5, 0));
    }

    #[test]
    fn pinned_iana_zone_keeps_its_wall_time_across_offset_changes() {
        let zone = timezone("America/New_York");
        let winter = next_due("FREQ=DAILY;BYHOUR=9", zone, utc(2026, 1, 15, 13, 0))
            .unwrap()
            .unwrap();
        let summer = next_due("FREQ=DAILY;BYHOUR=9", zone, utc(2026, 7, 15, 12, 0))
            .unwrap()
            .unwrap();

        assert_eq!(winter, utc(2026, 1, 15, 14, 0));
        assert_eq!(summer, utc(2026, 7, 15, 13, 0));
    }

    #[test]
    fn daily_schedule_crosses_leap_day_month_and_year_boundaries() {
        let leap_day = next_due(
            "FREQ=DAILY;BYHOUR=9",
            RoutineTimezone::Utc,
            utc(2028, 2, 28, 10, 0),
        )
        .unwrap()
        .unwrap();
        let new_year = next_due(
            "FREQ=DAILY;BYHOUR=9",
            RoutineTimezone::Utc,
            utc(2026, 12, 31, 10, 0),
        )
        .unwrap()
        .unwrap();

        assert_eq!(leap_day, utc(2028, 2, 29, 9, 0));
        assert_eq!(new_year, utc(2027, 1, 1, 9, 0));
    }

    #[test]
    fn weekly_schedule_crosses_a_year_boundary() {
        let next = next_due(
            "FREQ=WEEKLY;BYDAY=FR;BYHOUR=8",
            RoutineTimezone::Utc,
            utc(2026, 12, 31, 12, 0),
        )
        .unwrap()
        .unwrap();

        assert_eq!(next, utc(2027, 1, 1, 8, 0));
    }
}
