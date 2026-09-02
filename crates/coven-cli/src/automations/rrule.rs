//! Scoped RRULE parsing for routine schedules (coven#816).
//!
//! Only the recurrence vocabulary the acceptance criteria need is supported:
//! `FREQ=DAILY` and `FREQ=WEEKLY`, an optional `BYHOUR` list, and an optional
//! `BYDAY` list for weekly schedules. Anything else is refused, so a
//! definition can never silently schedule work the scheduler does not
//! understand.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RruleFrequency {
    Daily,
    Weekly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRrule {
    pub frequency: RruleFrequency,
    pub by_hour: Vec<u8>,
    pub by_day: Vec<String>,
}

fn parse_u8_list(raw: &str, label: &str, bound: u8) -> Result<Vec<u8>, String> {
    let mut values = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("{label} contains an empty entry"));
        }
        let parsed: u8 = part
            .parse()
            .map_err(|_| format!("{label} entry `{part}` is not an integer"))?;
        if parsed > bound {
            return Err(format!("{label} entry {parsed} exceeds {bound}"));
        }
        if values.contains(&parsed) {
            return Err(format!("{label} repeats entry {parsed}"));
        }
        values.push(parsed);
    }
    values.sort_unstable();
    Ok(values)
}

fn parse_by_day(raw: &str) -> Result<Vec<String>, String> {
    const ALLOWED: [&str; 14] = [
        "MO", "TU", "WE", "TH", "FR", "SA", "SU", "MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN",
    ];
    let mut days = BTreeSet::new();
    for part in raw.split(',') {
        let part = part.trim().to_ascii_uppercase();
        if !ALLOWED.contains(&part.as_str()) {
            return Err(format!("BYDAY entry `{part}` is not a weekday"));
        }
        // Canonicalize the two-letter form.
        days.insert(part[..2].to_string());
    }
    Ok(days.into_iter().collect())
}

pub fn parse_rrule(text: &str) -> Result<ParsedRrule, String> {
    let mut frequency: Option<RruleFrequency> = None;
    let mut by_hour: Option<Vec<u8>> = None;
    let mut by_day: Option<Vec<String>> = None;

    for part in text.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once('=') else {
            return Err(format!("rrule part `{part}` is not KEY=VALUE"));
        };
        let key = key.trim().to_ascii_uppercase();
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("rrule {key} has an empty value"));
        }
        match key.as_str() {
            "FREQ" => {
                if frequency.is_some() {
                    return Err("rrule FREQ must not be repeated".to_string());
                }
                frequency = Some(match value.to_ascii_uppercase().as_str() {
                    "DAILY" => RruleFrequency::Daily,
                    "WEEKLY" => RruleFrequency::Weekly,
                    other => {
                        return Err(format!("FREQ `{other}` is not supported (DAILY or WEEKLY)"));
                    }
                });
            }
            "BYHOUR" => {
                if by_hour.is_some() {
                    return Err("rrule BYHOUR must not be repeated".to_string());
                }
                by_hour = Some(parse_u8_list(value, "BYHOUR", 23)?);
            }
            "BYDAY" => {
                if by_day.is_some() {
                    return Err("rrule BYDAY must not be repeated".to_string());
                }
                by_day = Some(parse_by_day(value)?);
            }
            other => {
                return Err(format!("rrule key `{other}` is not supported"));
            }
        }
    }

    let frequency = frequency.ok_or_else(|| "rrule requires FREQ".to_string())?;
    let by_hour = by_hour.unwrap_or_else(|| vec![9]);
    if by_hour.is_empty() {
        return Err("BYHOUR must contain at least one hour".to_string());
    }
    let by_day = match frequency {
        RruleFrequency::Daily => {
            if by_day.is_some() {
                return Err("BYDAY is supported only for FREQ=WEEKLY".to_string());
            }
            Vec::new()
        }
        RruleFrequency::Weekly => {
            let days = by_day.unwrap_or_else(|| vec!["MO".to_string()]);
            if days.is_empty() {
                return Err("BYDAY must contain at least one weekday".to_string());
            }
            days
        }
    };

    Ok(ParsedRrule {
        frequency,
        by_hour,
        by_day,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_daily_with_hours() {
        let parsed = parse_rrule("FREQ=DAILY;BYHOUR=9,17").unwrap();
        assert_eq!(parsed.frequency, RruleFrequency::Daily);
        assert_eq!(parsed.by_hour, vec![9, 17]);
        assert!(parsed.by_day.is_empty());
    }

    #[test]
    fn defaults_byhour_to_nine() {
        let parsed = parse_rrule("FREQ=DAILY").unwrap();
        assert_eq!(parsed.by_hour, vec![9]);
    }

    #[test]
    fn parses_weekly_with_days() {
        let parsed = parse_rrule("FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=8").unwrap();
        assert_eq!(parsed.frequency, RruleFrequency::Weekly);
        assert_eq!(parsed.by_day, vec!["FR", "MO", "WE"]);
        assert_eq!(parsed.by_hour, vec![8]);
    }

    #[test]
    fn rejects_unsupported_frequency() {
        let error = parse_rrule("FREQ=HOURLY").unwrap_err();
        assert!(error.contains("not supported"), "{error}");
    }

    #[test]
    fn rejects_unsupported_key() {
        let error = parse_rrule("FREQ=DAILY;COUNT=3").unwrap_err();
        assert!(error.contains("COUNT"), "{error}");
    }

    #[test]
    fn rejects_out_of_range_hour() {
        let error = parse_rrule("FREQ=DAILY;BYHOUR=24").unwrap_err();
        assert!(error.contains("exceeds 23"), "{error}");
    }

    #[test]
    fn rejects_unknown_weekday() {
        let error = parse_rrule("FREQ=WEEKLY;BYDAY=XX").unwrap_err();
        assert!(error.contains("not a weekday"), "{error}");
    }

    #[test]
    fn rejects_byday_for_daily_schedules() {
        let error = parse_rrule("FREQ=DAILY;BYDAY=MO;BYHOUR=9").unwrap_err();
        assert!(error.contains("BYDAY is supported only"), "{error}");
    }

    #[test]
    fn rejects_duplicate_supported_parts() {
        for rule in [
            "FREQ=DAILY;FREQ=WEEKLY;BYDAY=MO",
            "FREQ=DAILY;BYHOUR=9;BYHOUR=17",
            "FREQ=WEEKLY;BYDAY=MO;BYDAY=TU;BYHOUR=9",
        ] {
            let error = parse_rrule(rule).unwrap_err();
            assert!(error.contains("must not be repeated"), "{rule}: {error}");
        }
    }

    #[test]
    fn rejects_missing_frequency() {
        let error = parse_rrule("BYHOUR=9").unwrap_err();
        assert!(error.contains("requires FREQ"), "{error}");
    }
}
