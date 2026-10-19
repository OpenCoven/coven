//! Windows command-line encoding without a shell or lossy UTF-16 conversion.
use crate::Failure;

pub const MAX_ARGUMENTS: usize = 16;
pub const MAX_ARGUMENT_UNITS: usize = 8192;
pub const MAX_COMMAND_UNITS: usize = 32767;

pub fn quote_argument(value: &[u16]) -> Result<Vec<u16>, Failure> {
    if value.len() > MAX_ARGUMENT_UNITS || value.contains(&0) {
        return Err(Failure::InvalidLaunch);
    }
    let mut quoted = Vec::with_capacity(value.len() * 2 + 2);
    quoted.push(34);
    let mut slashes = 0;
    for &unit in value {
        if unit == 92 {
            slashes += 1;
            continue;
        }
        let count = if unit == 34 { slashes * 2 + 1 } else { slashes };
        quoted.extend(std::iter::repeat_n(92, count));
        quoted.push(unit);
        slashes = 0;
    }
    quoted.extend(std::iter::repeat_n(92, slashes * 2));
    quoted.push(34);
    Ok(quoted)
}

pub fn command_line(program: &[u16], args: &[String]) -> Result<Vec<u16>, Failure> {
    if program.is_empty() || args.len() > MAX_ARGUMENTS {
        return Err(Failure::InvalidLaunch);
    }
    let mut line = quote_argument(program)?;
    for arg in args {
        let units: Vec<u16> = arg.encode_utf16().collect();
        let quoted = quote_argument(&units)?;
        if line.len() + quoted.len() + 2 > MAX_COMMAND_UNITS {
            return Err(Failure::InvalidLaunch);
        }
        line.push(32);
        line.extend(quoted);
    }
    if line.len() + 1 > MAX_COMMAND_UNITS {
        return Err(Failure::InvalidLaunch);
    }
    line.push(0);
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::{
        command_line, quote_argument, MAX_ARGUMENTS, MAX_ARGUMENT_UNITS, MAX_COMMAND_UNITS,
    };
    use crate::Failure;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    #[test]
    fn quotes_empty_spaces_quotes_and_trailing_backslashes() {
        for (input, expected) in [
            ("", "\"\""),
            ("plain", "\"plain\""),
            ("a b", "\"a b\""),
            ("a\"b", "\"a\\\"b\""),
            ("C:\\tail\\", "\"C:\\tail\\\\\""),
        ] {
            assert_eq!(quote_argument(&wide(input)).unwrap(), wide(expected));
        }
        assert_eq!(
            quote_argument(&[97, 92, 34, 98]).unwrap(),
            [34, 97, 92, 92, 92, 34, 98, 34]
        );
    }

    #[test]
    fn preserves_opaque_utf16_program_units() {
        assert_eq!(
            quote_argument(&[0xd800, 0xdc00, 0xd800]).unwrap(),
            [34, 0xd800, 0xdc00, 0xd800, 34]
        );
    }

    #[test]
    fn command_line_has_one_final_nul_and_no_shell_interpretation() {
        let line = command_line(
            &wide("C:\\Program Files\\target.exe"),
            &["".into(), "& echo PRIVATE".into(), "tail\\".into()],
        )
        .unwrap();
        assert_eq!(line.iter().filter(|&&unit| unit == 0).count(), 1);
        assert_eq!(line.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            "\"C:\\Program Files\\target.exe\" \"\" \"& echo PRIVATE\" \"tail\\\\\""
        );
    }

    #[test]
    fn refuses_nul_empty_program_and_excessive_arguments() {
        assert_eq!(quote_argument(&[1, 0, 2]), Err(Failure::InvalidLaunch));
        assert_eq!(command_line(&[], &[]), Err(Failure::InvalidLaunch));
        assert_eq!(
            command_line(&wide("p"), &vec!["x".into(); MAX_ARGUMENTS + 1]),
            Err(Failure::InvalidLaunch)
        );
    }

    #[test]
    fn enforces_argument_and_expanded_command_bounds() {
        assert!(quote_argument(&vec![97; MAX_ARGUMENT_UNITS]).is_ok());
        assert_eq!(
            quote_argument(&vec![97; MAX_ARGUMENT_UNITS + 1]),
            Err(Failure::InvalidLaunch)
        );
        let long = "a".repeat(MAX_ARGUMENT_UNITS);
        assert_eq!(
            command_line(
                &wide("p"),
                &[long.clone(), long.clone(), long.clone(), long]
            ),
            Err(Failure::InvalidLaunch)
        );
        let escaping = "\\".repeat(MAX_ARGUMENT_UNITS);
        assert_eq!(
            command_line(&wide("p"), &[escaping.clone(), escaping]),
            Err(Failure::InvalidLaunch)
        );
    }

    #[test]
    fn accepts_exact_command_limit_and_refuses_the_next_unit() {
        let long = "a".repeat(MAX_ARGUMENT_UNITS);
        let mut args = vec![long.clone(), long.clone(), long, "a".repeat(8175)];
        assert_eq!(
            command_line(&wide("p"), &args).unwrap().len(),
            MAX_COMMAND_UNITS
        );
        args[3].push('a');
        assert_eq!(command_line(&wide("p"), &args), Err(Failure::InvalidLaunch));
    }
}
