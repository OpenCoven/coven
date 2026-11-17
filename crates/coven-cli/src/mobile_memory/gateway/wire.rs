//! Private inherited-pipe framing. Bodies remain raw UTF-8 bytes, preserving
//! the daemon's 4 MiB body allowance even for maximally escaped JSON strings.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub(super) const PROTOCOL: &str = "coven.mobile-worker.v1";
const MAGIC: &[u8] = b"\nCOVEN_MOBILE_WORKER_V1\n";
const MAX_HEADER: usize = 16 * 1024;
pub(super) const MAX_BODY: usize = crate::daemon::MAX_SOCKET_BODY_BYTES;
pub(super) const MAX_FAILURE_BYTES: usize = 64 * 1024;
const MAX_PREAMBLE: usize = 4096;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Header {
    Hello {
        protocol: String,
        build_commit: String,
    },
    Start,
    Ready {
        enabled: bool,
    },
    StartupFailure {
        config_validation: bool,
    },
    Request {
        id: u64,
        method: String,
        path: String,
        has_body: bool,
    },
    Response {
        id: u64,
        status: u16,
    },
    Unavailable {
        id: u64,
    },
    Failure {
        id: u64,
    },
    Stop,
}

#[derive(Debug)]
pub(super) struct Frame {
    pub header: Header,
    pub body: Vec<u8>,
}

impl Frame {
    pub(super) fn control(header: Header) -> Self {
        Self {
            header,
            body: Vec::new(),
        }
    }
}

pub(super) fn write_magic(writer: &mut impl Write) -> Result<()> {
    writer.write_all(MAGIC)?;
    writer.flush()?;
    Ok(())
}

pub(super) fn read_magic(reader: &mut impl Read, allow_test_preamble: bool) -> Result<()> {
    if !allow_test_preamble {
        let mut magic = vec![0; MAGIC.len()];
        reader.read_exact(&mut magic)?;
        ensure!(magic == MAGIC, "mobile worker protocol marker mismatch");
        return Ok(());
    }
    // libtest emits a bounded harness banner before the explicitly selected
    // ignored fixture. Production disallows this prefix entirely.
    let mut window = Vec::with_capacity(MAGIC.len());
    for _ in 0..MAX_PREAMBLE + MAGIC.len() {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        if window.len() == MAGIC.len() {
            window.remove(0);
        }
        window.push(byte[0]);
        if window == MAGIC {
            return Ok(());
        }
    }
    anyhow::bail!("mobile worker protocol preamble exceeds bound")
}

/// Validate caller-controlled fields before admitting a request to the pipe.
/// Rejection here must not become a fatal transport error for other callers.
pub(super) fn validate_frame(frame: &Frame) -> Result<Vec<u8>> {
    let header = serde_json::to_vec(&frame.header)?;
    ensure!(
        header.len() <= MAX_HEADER,
        "mobile worker header exceeds bound"
    );
    ensure!(
        frame.body.len() <= MAX_BODY,
        "mobile worker body exceeds bound"
    );
    validate_body(&frame.header, frame.body.len())?;
    Ok(header)
}

pub(super) fn write_frame(writer: &mut impl Write, frame: &Frame) -> Result<()> {
    let header = validate_frame(frame)?;
    writer.write_all(&(header.len() as u32).to_be_bytes())?;
    writer.write_all(&header)?;
    writer.write_all(&(frame.body.len() as u32).to_be_bytes())?;
    writer.write_all(&frame.body)?;
    writer.flush()?;
    Ok(())
}

pub(super) fn read_frame(reader: &mut impl Read) -> Result<Frame> {
    let header_size = read_length(reader)?;
    ensure!(
        header_size <= MAX_HEADER,
        "mobile worker header exceeds bound"
    );
    let mut header = vec![0; header_size];
    reader.read_exact(&mut header)?;
    let header = serde_json::from_slice(&header).context("invalid mobile worker header")?;
    let body_size = read_length(reader)?;
    ensure!(body_size <= MAX_BODY, "mobile worker body exceeds bound");
    validate_body(&header, body_size)?;
    let mut body = vec![0; body_size];
    reader.read_exact(&mut body)?;
    Ok(Frame { header, body })
}

fn read_length(reader: &mut impl Read) -> Result<usize> {
    let mut value = [0; 4];
    reader.read_exact(&mut value)?;
    Ok(u32::from_be_bytes(value) as usize)
}

fn validate_body(header: &Header, size: usize) -> Result<()> {
    match header {
        Header::Request { has_body: true, .. } | Header::Response { .. } => Ok(()),
        Header::Failure { .. } | Header::StartupFailure { .. } => {
            ensure!(
                size <= MAX_FAILURE_BYTES,
                "mobile worker failure diagnostic exceeds bound"
            );
            Ok(())
        }
        _ => {
            ensure!(size == 0, "unexpected mobile worker body");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_maximum_body_does_not_expand() {
        let frame = Frame {
            header: Header::Request {
                id: 1,
                method: "POST".into(),
                path: "/api/v1/internal/mobile/pairings".into(),
                has_body: true,
            },
            body: vec![b'\\'; 4 * 1024 * 1024],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        assert!(bytes.len() < 4 * 1024 * 1024 + MAX_HEADER);
        let decoded = read_frame(&mut &bytes[..]).unwrap();
        assert_eq!(decoded.header, frame.header);
        assert_eq!(decoded.body, frame.body);
    }
    #[test]
    fn oversize_lengths_fail_before_reading_payload() {
        assert!(read_frame(&mut &((MAX_HEADER + 1) as u32).to_be_bytes()[..]).is_err());
        let header = serde_json::to_vec(&Header::Response { id: 1, status: 200 }).unwrap();
        let mut bytes = Vec::new();
        bytes.extend((header.len() as u32).to_be_bytes());
        bytes.extend(header);
        bytes.extend(((4 * 1024 * 1024 + 1) as u32).to_be_bytes());
        assert!(read_frame(&mut &bytes[..])
            .unwrap_err()
            .to_string()
            .contains("body exceeds bound"));
        let oversized = Frame {
            header: Header::Response { id: 1, status: 200 },
            body: vec![b'x'; 4 * 1024 * 1024 + 1],
        };
        assert!(write_frame(&mut Vec::new(), &oversized).is_err());
    }
    #[test]
    fn failure_diagnostic_is_raw_and_has_its_own_bound() {
        let frame = Frame {
            header: Header::Failure { id: 7 },
            body: vec![b'\\'; 64 * 1024],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        assert_eq!(read_frame(&mut &bytes[..]).unwrap().body, frame.body);
        let oversized = Frame {
            header: Header::Failure { id: 7 },
            body: vec![b'x'; 64 * 1024 + 1],
        };
        assert!(write_frame(&mut Vec::new(), &oversized).is_err());
    }
    #[test]
    fn bounded_fixture_preamble_and_control_body_rejection() {
        let mut bytes = b"running 1 test\n".to_vec();
        bytes.extend(MAGIC);
        assert!(read_magic(&mut &bytes[..], true).is_ok());
        assert!(read_magic(&mut &bytes[..], false).is_err());
        assert!(read_magic(&mut &vec![b'x'; MAX_PREAMBLE + MAGIC.len()][..], true).is_err());
        assert!(write_frame(
            &mut Vec::new(),
            &Frame {
                header: Header::Stop,
                body: vec![1]
            }
        )
        .is_err());
    }
}
