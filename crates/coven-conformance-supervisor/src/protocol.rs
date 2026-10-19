//! Bounded helper input framing. Blocking I/O requires independent native lease supervision.
use crate::Failure;
use std::io::{BufRead, Read, Write};

pub const MAX_HEADER_BYTES: usize = 8192;
pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TIMEOUT_MS: u32 = 600_000;

#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub nonce: String,
    pub subject_sha256: String,
    pub args: Vec<String>,
    pub input_bytes: u64,
    pub timeout_ms: u32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LaunchHeader {
    schema_version: String,
    kind: String,
    nonce: String,
    subject_sha256: String,
    args: Vec<String>,
    input_bytes: u64,
    timeout_ms: u32,
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn read_launch(reader: &mut impl BufRead) -> Result<Launch, Failure> {
    // Take caps materialization and consumption even when no delimiter arrives.
    // The BufRead owner retains any already-buffered binary body after this line.
    let mut bytes = Vec::new();
    reader
        .take((MAX_HEADER_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| Failure::InvalidLaunch)?;
    // Serde's struct visitor also accepts positional arrays; require object syntax.
    if bytes.last() != Some(&b'\n') || bytes.trim_ascii_start().first() != Some(&b'{') {
        return Err(Failure::InvalidLaunch);
    }
    let header: LaunchHeader =
        serde_json::from_slice(&bytes[..bytes.len() - 1]).map_err(|_| Failure::InvalidLaunch)?;
    if header.schema_version != crate::ABI
        || header.kind != "launch"
        || !lower_hex(&header.nonce, 32)
        || !lower_hex(&header.subject_sha256, 64)
        || header.input_bytes > MAX_INPUT_BYTES
        || header.timeout_ms == 0
        || header.timeout_ms > MAX_TIMEOUT_MS
        || header.args.len() > crate::command::MAX_ARGUMENTS
        || header.args.iter().any(|arg| {
            arg.contains('\0') || arg.encode_utf16().count() > crate::command::MAX_ARGUMENT_UNITS
        })
    {
        return Err(Failure::InvalidLaunch);
    }
    Ok(Launch {
        nonce: header.nonce,
        subject_sha256: header.subject_sha256,
        args: header.args,
        input_bytes: header.input_bytes,
        timeout_ms: header.timeout_ms,
    })
}

/// Forward only the declared body, then require EOF. The caller closes target stdin
/// after this returns. Native lease/timeout supervision must run independently:
/// this function makes no wall-clock guarantee for an arbitrary blocking reader/writer.
pub fn forward_input(
    reader: &mut impl Read,
    writer: &mut impl Write,
    bytes: u64,
) -> Result<(), Failure> {
    if bytes > MAX_INPUT_BYTES {
        return Err(Failure::InvalidLaunch);
    }
    let mut remaining = bytes;
    let mut buffer = [0; 8192];
    while remaining > 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        reader
            .read_exact(&mut buffer[..count])
            .map_err(|_| Failure::InvalidLaunch)?;
        writer
            .write_all(&buffer[..count])
            .map_err(|_| Failure::InvalidLaunch)?;
        remaining -= count as u64;
    }
    loop {
        match reader.read(&mut buffer[..1]) {
            Ok(0) => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            _ => return Err(Failure::InvalidLaunch),
        }
    }
    writer.flush().map_err(|_| Failure::InvalidLaunch)
}

/// Consume exactly the acknowledgement, leaving the descriptor open as a lease.
/// A separate native reader must treat later data, EOF or error as lease failure.
pub fn read_ack(reader: &mut impl Read, nonce: &str) -> Result<(), Failure> {
    if !lower_hex(nonce, 32) {
        return Err(Failure::InvalidLaunch);
    }
    let mut frame = [0; 36];
    reader
        .read_exact(&mut frame)
        .map_err(|_| Failure::InvalidLaunch)?;
    if &frame[..3] != b"GO " || &frame[3..35] != nonce.as_bytes() || frame[35] != b'\n' {
        return Err(Failure::InvalidLaunch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::{self, BufReader, Cursor};

    fn header() -> Value {
        json!({"schemaVersion": crate::ABI, "kind":"launch", "nonce":"a".repeat(32),
            "subjectSha256":"b".repeat(64), "args":["plain", "", "& private"],
            "inputBytes":3,"timeoutMs":1000})
    }
    fn framed(value: &Value) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        bytes
    }
    fn refused(bytes: &[u8]) {
        assert_eq!(
            read_launch(&mut Cursor::new(bytes)),
            Err(Failure::InvalidLaunch)
        );
    }

    #[test]
    fn reads_closed_launch_and_leaves_binary_body_untouched() {
        let mut bytes = framed(&header());
        bytes.extend_from_slice(&[0, 255, b'\n']);
        let mut reader = BufReader::with_capacity(4096, Cursor::new(bytes));
        let launch = read_launch(&mut reader).unwrap();
        assert_eq!(
            launch,
            Launch {
                nonce: "a".repeat(32),
                subject_sha256: "b".repeat(64),
                args: vec!["plain".into(), "".into(), "& private".into()],
                input_bytes: 3,
                timeout_ms: 1000
            }
        );
        let mut body = Vec::new();
        reader.read_to_end(&mut body).unwrap();
        assert_eq!(body, [0, 255, b'\n']);
    }

    #[test]
    fn accepts_exact_header_limit_but_refuses_next_byte_without_reading_body() {
        let mut bytes = serde_json::to_vec(&header()).unwrap();
        bytes.resize(MAX_HEADER_BYTES, b' ');
        bytes.push(b'\n');
        let mut reader = Cursor::new(bytes.clone());
        assert!(read_launch(&mut reader).is_ok());
        assert_eq!(reader.position(), (MAX_HEADER_BYTES + 1) as u64);
        bytes.insert(MAX_HEADER_BYTES, b' ');
        let mut reader = Cursor::new(bytes);
        assert_eq!(read_launch(&mut reader), Err(Failure::InvalidLaunch));
        assert_eq!(reader.position(), (MAX_HEADER_BYTES + 1) as u64);
    }

    #[test]
    fn rejects_every_missing_newline_truncation_and_malformed_header() {
        let bytes = framed(&header());
        for end in 0..bytes.len() {
            refused(&bytes[..end]);
        }
        for bytes in [
            b"\xff\n".as_slice(),
            b"[]\n",
            b"null\n",
            b"{}\n",
            b"{\n",
            b"{}{}\n",
        ] {
            refused(bytes);
        }
    }

    #[test]
    fn rejects_unknown_duplicate_missing_and_wrong_type_fields() {
        let value = header();
        for key in value.as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(key);
            refused(&framed(&missing));
            let mut wrong = value.clone();
            wrong[key] = json!({"private":"payload"});
            refused(&framed(&wrong));
        }
        let mut extra = value.clone();
        extra["path"] = json!("C:\\private");
        refused(&framed(&extra));
        let text = serde_json::to_string(&value).unwrap();
        refused(format!("{{\"kind\":\"launch\",{}\n", &text[1..]).as_bytes());
    }

    #[test]
    fn refuses_complete_positional_array_but_accepts_object_whitespace() {
        let positional = json!([
            crate::ABI,
            "launch",
            "a".repeat(32),
            "b".repeat(64),
            [],
            0,
            1
        ]);
        refused(&framed(&positional));
        let mut object = b" \t\r".to_vec();
        object.extend(framed(&header()));
        assert!(read_launch(&mut Cursor::new(object)).is_ok());
    }

    #[test]
    fn validates_identity_integer_ranges_and_argument_limits() {
        for (key, value) in [
            ("schemaVersion", json!("other")),
            ("kind", json!("ready")),
            ("nonce", json!("A".repeat(32))),
            ("nonce", json!("a".repeat(31))),
            ("subjectSha256", json!("g".repeat(64))),
            ("subjectSha256", json!("b".repeat(65))),
            ("inputBytes", json!(MAX_INPUT_BYTES + 1)),
            ("inputBytes", json!(-1)),
            ("inputBytes", json!(1.5)),
            ("timeoutMs", json!(0)),
            ("timeoutMs", json!(MAX_TIMEOUT_MS + 1)),
            ("args", json!(["hidden\u{0}suffix"])),
            ("args", json!(vec![""; crate::command::MAX_ARGUMENTS + 1])),
        ] {
            let mut h = header();
            h[key] = value;
            refused(&framed(&h));
        }
        for (input, timeout) in [(0, 1), (MAX_INPUT_BYTES, MAX_TIMEOUT_MS as u64)] {
            let mut h = header();
            h["inputBytes"] = json!(input);
            h["timeoutMs"] = json!(timeout);
            assert!(read_launch(&mut Cursor::new(framed(&h))).is_ok());
        }
    }

    #[test]
    fn forwards_exact_binary_body_and_refuses_truncated_or_surplus_input() {
        for body in [vec![], vec![0, 255, b'\n'], vec![42; 65537]] {
            let mut output = Vec::new();
            forward_input(&mut Cursor::new(&body), &mut output, body.len() as u64).unwrap();
            assert_eq!(output, body);
        }
        for (body, count) in [(vec![1, 2], 3), (vec![1, 2, 3, 4], 3), (vec![1], 0)] {
            assert_eq!(
                forward_input(&mut Cursor::new(body), &mut Vec::new(), count),
                Err(Failure::InvalidLaunch)
            );
        }
        let mut reader = Cursor::new([1]);
        assert_eq!(
            forward_input(&mut reader, &mut Vec::new(), MAX_INPUT_BYTES + 1),
            Err(Failure::InvalidLaunch)
        );
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn acknowledgement_reads_exact_nonce_frame_and_leaves_live_lease_open() {
        let nonce = "a".repeat(32);
        let frame = format!("GO {nonce}\n");
        let mut reader = Cursor::new(format!("{frame}unexpected"));
        read_ack(&mut reader, &nonce).unwrap();
        assert_eq!(reader.position(), 36);
        for end in 0..frame.len() {
            assert_eq!(
                read_ack(&mut Cursor::new(&frame.as_bytes()[..end]), &nonce),
                Err(Failure::InvalidLaunch)
            );
        }
        for invalid in [
            format!("GO {}\n", "b".repeat(32)),
            format!("GO {nonce}\r"),
            format!("NO {nonce}\n"),
        ] {
            assert_eq!(
                read_ack(&mut Cursor::new(invalid), &nonce),
                Err(Failure::InvalidLaunch)
            );
        }
        assert_eq!(
            read_ack(&mut Cursor::new(frame), "invalid"),
            Err(Failure::InvalidLaunch)
        );
    }

    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("PRIVATE path payload"))
        }
    }
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("PRIVATE path payload"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("PRIVATE path payload"))
        }
    }
    #[test]
    fn io_failures_have_only_fixed_minimized_categories() {
        let failure = read_launch(&mut BufReader::new(Broken)).unwrap_err();
        assert_eq!(failure.to_string(), "launch_invalid");
        assert_eq!(
            read_ack(&mut Broken, &"a".repeat(32)),
            Err(Failure::InvalidLaunch)
        );
        assert_eq!(
            forward_input(&mut Broken, &mut Vec::new(), 1),
            Err(Failure::InvalidLaunch)
        );
        assert_eq!(
            forward_input(&mut Cursor::new([1]), &mut Broken, 1),
            Err(Failure::InvalidLaunch)
        );
    }
}
