//! Bounded output forwarding for independently supervised native I/O workers.
//! Generic blocking I/O has no deadline here; the native coordinator owns cancellation.
use crate::control::Refusal;
use std::{
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

pub fn output(
    reader: &mut impl Read,
    writer: &mut impl Write,
    stopping: &AtomicBool,
) -> Result<(), Refusal> {
    let mut remaining = MAX_OUTPUT_BYTES;
    let mut buffer = [0; 8192];
    loop {
        if stopping.load(Ordering::Acquire) {
            return Err(Refusal::IoRefused);
        }
        // Only one overflow sentinel may be consumed, even at the exact cap.
        let requested = buffer.len().min(remaining + 1);
        let count = match reader.read(&mut buffer[..requested]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Refusal::IoRefused),
            Ok(count) if count > requested => return Err(Refusal::IoRefused),
            Ok(count) => count,
        };
        if count == 0 {
            if stopping.load(Ordering::Acquire) {
                return Err(Refusal::IoRefused);
            }
            writer.flush().map_err(|_| Refusal::IoRefused)?;
            return if stopping.load(Ordering::Acquire) {
                Err(Refusal::IoRefused)
            } else {
                Ok(())
            };
        }
        if count > remaining {
            return Err(Refusal::OutputLimitExceeded);
        }
        let mut pending = &buffer[..count];
        while !pending.is_empty() {
            if stopping.load(Ordering::Acquire) {
                return Err(Refusal::IoRefused);
            }
            match writer.write(pending) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) | Ok(0) => return Err(Refusal::IoRefused),
                Ok(written) if written > pending.len() => return Err(Refusal::IoRefused),
                Ok(written) => pending = &pending[written..],
            }
        }
        remaining -= count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, sync::atomic::Ordering};

    #[test]
    fn forwards_empty_binary_and_exact_limit_without_changing_bytes() {
        for bytes in [
            Vec::new(),
            (0..=255).collect(),
            vec![0x9f; MAX_OUTPUT_BYTES],
        ] {
            let mut sent = Vec::new();
            output(&mut bytes.as_slice(), &mut sent, &AtomicBool::new(false)).unwrap();
            assert_eq!(sent, bytes);
        }
    }

    #[test]
    fn overflow_consumes_only_one_sentinel_and_never_forwards_excess() {
        let mut source = io::Cursor::new(vec![0x41; MAX_OUTPUT_BYTES + 2048]);
        let mut sent = Vec::new();
        assert_eq!(
            output(&mut source, &mut sent, &AtomicBool::new(false)),
            Err(Refusal::OutputLimitExceeded)
        );
        assert_eq!(source.position(), (MAX_OUTPUT_BYTES + 1) as u64);
        assert!(sent.len() <= MAX_OUTPUT_BYTES);
    }

    struct PartialWriter {
        bytes: Vec<u8>,
        interrupted: bool,
        flushes: usize,
    }
    impl Write for PartialWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = bytes.len().min(3);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }
    #[test]
    fn retries_partial_writes_and_interrupts_without_losing_output() {
        let bytes: Vec<u8> = (0..=255).cycle().take(20001).collect();
        let mut sink = PartialWriter {
            bytes: Vec::new(),
            interrupted: false,
            flushes: 0,
        };
        output(&mut bytes.as_slice(), &mut sink, &AtomicBool::new(false)).unwrap();
        assert_eq!(sink.bytes, bytes);
        assert_eq!(sink.flushes, 1);
    }

    struct Fault {
        read: bool,
        write: bool,
        flush: bool,
    }
    impl Read for Fault {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            assert!(self.read);
            Err(io::Error::other("PRIVATE_TARGET_READ"))
        }
    }
    impl Write for Fault {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.write {
                Err(io::Error::other("PRIVATE_TARGET_WRITE"))
            } else {
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.flush {
                Err(io::Error::other("PRIVATE_TARGET_FLUSH"))
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn read_write_and_flush_errors_are_fixed_refusals() {
        let stopping = AtomicBool::new(false);
        assert_eq!(
            output(
                &mut Fault {
                    read: true,
                    write: false,
                    flush: false
                },
                &mut Vec::new(),
                &stopping
            ),
            Err(Refusal::IoRefused)
        );
        for (write, flush) in [(true, false), (false, true)] {
            assert_eq!(
                output(
                    &mut &b"x"[..],
                    &mut Fault {
                        read: false,
                        write,
                        flush
                    },
                    &stopping
                ),
                Err(Refusal::IoRefused)
            );
        }
    }

    struct Cancels<'a>(&'a AtomicBool);
    impl Read for Cancels<'_> {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.store(true, Ordering::Release);
            Err(io::ErrorKind::Interrupted.into())
        }
    }
    impl Write for Cancels<'_> {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            self.0.store(true, Ordering::Release);
            Err(io::ErrorKind::Interrupted.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("cancelled output must not flush")
        }
    }
    #[test]
    fn interrupted_read_and_write_retries_observe_cancellation() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            output(&mut Cancels(&stop), &mut Vec::new(), &stop),
            Err(Refusal::IoRefused)
        );
        stop.store(false, Ordering::Release);
        assert_eq!(
            output(&mut &b"x"[..], &mut Cancels(&stop), &stop),
            Err(Refusal::IoRefused)
        );
    }

    struct ForbiddenIo;
    impl Read for ForbiddenIo {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("cancelled output read")
        }
    }
    impl Write for ForbiddenIo {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            panic!("cancelled output write")
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("cancelled output flush")
        }
    }
    #[test]
    fn cancellation_before_start_performs_no_io() {
        assert_eq!(
            output(&mut ForbiddenIo, &mut ForbiddenIo, &AtomicBool::new(true)),
            Err(Refusal::IoRefused)
        );
    }
}
