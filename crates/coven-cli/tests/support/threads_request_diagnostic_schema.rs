// Extracted from the existing four-field consumer so both platform schemas can
// be exercised without a daemon or a native Windows host.
pub(crate) fn native_fields_valid(fields: &[&str], windows: bool) -> bool {
    if !windows {
        return fields.len() == 4;
    }
    if fields.len() != 7 && fields.len() != 8 {
        return false;
    }
    let number = |index: usize, prefix: &str, min: u64, max: u64| {
        fields[index].strip_prefix(prefix).is_some_and(|value| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u64>().is_ok_and(|value| value >= min && value <= max)
        })
    };
    number(4, "native_pid=", 1, u32::MAX.into())
        && number(5, "native_tid=", 1, u32::MAX.into())
        && if fields.len() == 7 {
            fields[6] == "native_clock=unavailable"
        } else {
            number(6, "native_qpc=", 0, i64::MAX as u64)
                && number(7, "native_qpc_hz=", 1, i64::MAX as u64)
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "request=1 phase=stage-begin elapsed_us=0 prior_observer_us=0";
    const NATIVE: &str = "native_pid=11 native_tid=13 native_qpc=17 native_qpc_hz=19";

    fn valid(record: &str, windows: bool) -> bool {
        native_fields_valid(&record.split_whitespace().collect::<Vec<_>>(), windows)
    }

    #[test]
    fn accepts_only_platform_specific_native_schemas() {
        assert!(valid(BASE, false));
        assert!(!valid(BASE, true));
        assert!(valid(&format!("{BASE} {NATIVE}"), true));
        assert!(valid(&format!("{BASE} native_pid=11 native_tid=13 native_clock=unavailable"), true));
        assert!(!valid(&format!("{BASE} {NATIVE}"), false));
    }

    #[test]
    fn rejects_private_extra_or_malformed_native_fields() {
        let good = format!("{BASE} {NATIVE}");
        for bad in [
            format!("{good} private_path=synthetic-private"),
            good.replace("native_pid=11", "native_pid=0"),
            good.replace("native_tid=13", "native_tid=4294967296"),
            good.replace("native_qpc=17", "native_qpc=-1"),
            good.replace("native_qpc_hz=19", "native_qpc_hz=0"),
            good.replace("native_qpc=17", "native_qpc=+0"),
            good.replace("native_qpc=17", "private_field=17"),
            format!("{BASE} native_pid=11 native_tid=13 native_clock=other"),
        ] {
            assert!(!valid(&bad, true), "accepted malformed diagnostic: {bad}");
        }
    }
}
