//! Payload-free standalone panic identification. Never an acceptance status.

/// Encode an allowlisted *compiled* source component and bounded line number.
/// Never copy a panic payload, private build path, target argument or backtrace.
/// Unknown sources deliberately lose even their line number.
pub fn exit_code(file: &str, line: u32) -> u32 {
    const SOURCES: [&str; 14] = [
        "crates/coven-conformance-supervisor/src/bin/coven-conformance-supervisor.rs",
        "crates/coven-conformance-supervisor/src/lease/native.rs",
        "crates/coven-conformance-supervisor/src/coordinator.rs",
        "crates/coven-conformance-supervisor/src/worker.rs",
        "crates/coven-conformance-supervisor/src/pipe.rs",
        "crates/coven-conformance-supervisor/src/job.rs",
        "crates/coven-conformance-supervisor/src/image/native.rs",
        "crates/coven-conformance-supervisor/src/image.rs",
        "crates/coven-conformance-supervisor/src/protocol.rs",
        "crates/coven-conformance-supervisor/src/command.rs",
        "crates/coven-conformance-supervisor/src/control.rs",
        "crates/coven-conformance-supervisor/src/transfer.rs",
        "crates/coven-conformance-supervisor/src/panic_site.rs",
        "crates/coven-conformance-supervisor/tests/native_helper.rs",
    ];
    for (index, source) in SOURCES.iter().enumerate() {
        // Both separators occur in compiler locations. Match the whole crate
        // suffix without allocation, even on a panic path.
        if file.len() >= source.len()
            && file
                .bytes()
                .rev()
                .zip(source.bytes().rev())
                .all(|(actual, expected)| (if actual == b'\\' { b'/' } else { actual }) == expected)
        {
            return 0xe0c0_0000 | (((index + 1) as u32) << 16) | line.min(0xffff);
        }
    }
    0xe0c0_0000
}

/// Standalone process only: do not block on a controller pipe while panicking.
/// The numeric status is always a failure with cleanup unproven. Process exit
/// closes owned kill-on-close Jobs, but is never accepted as cleanup evidence.
#[cfg(windows)]
pub fn install_hook() {
    std::panic::set_hook(Box::new(|info| {
        let code = info.location().map_or(0xe0c0_0000, |location| {
            exit_code(location.file(), location.line())
        });
        // SAFETY: This hook belongs only to the standalone helper or its private
        // fixture. No arbitrary host embeds it; terminate rather than unwind
        // through pending native I/O or wait on blocked diagnostic output.
        unsafe {
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            TerminateProcess(GetCurrentProcess(), code);
        }
        // Successful self-termination does not return. If it fails, retain a
        // failing process exit without unwinding or emitting a panic payload.
        std::process::abort();
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_only_allowlisted_compiled_source_locations() {
        assert_eq!(
            exit_code(
                "crates/coven-conformance-supervisor/src/coordinator.rs",
                127
            ),
            0xe0c3_007f
        );
        assert_eq!(
            exit_code(
                r"crates\coven-conformance-supervisor\src\lease\native.rs",
                80
            ),
            0xe0c2_0050
        );
        assert_eq!(
            exit_code(
                "/private/build/crates/coven-conformance-supervisor/src/pipe.rs",
                143
            ),
            0xe0c5_008f
        );
    }

    #[test]
    fn unknown_locations_have_one_fixed_status_and_no_line_information() {
        for private in [
            "",
            "/private/customer/project.rs",
            "coordinator.rs",
            "private-value",
        ] {
            assert_eq!(exit_code(private, 1789), 0xe0c0_0000);
        }
    }

    #[test]
    fn line_overflow_does_not_change_the_source_component() {
        assert_eq!(
            exit_code("crates/coven-conformance-supervisor/src/job.rs", u32::MAX),
            0xe0c6_ffff
        );
    }
}
