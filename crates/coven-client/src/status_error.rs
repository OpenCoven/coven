use crate::ClientError;

#[derive(Clone, Copy, Debug)]
pub(crate) enum StatusWriteStage {
    CreateTemporary,
    WriteContents,
    WriteNewline,
    SyncTemporary,
    ConvertDescriptor,
    ApplySecurity,
    OpenToken,
    ReadToken,
    ReplaceStatus,
}

impl StatusWriteStage {
    pub(crate) fn io_error(self, source: std::io::Error) -> ClientError {
        ClientError::Io {
            operation: match self {
                Self::CreateTemporary => {
                    "failed to write owner-only Windows daemon status: create-temporary-file"
                }
                Self::WriteContents => {
                    "failed to write owner-only Windows daemon status: write-contents"
                }
                Self::WriteNewline => {
                    "failed to write owner-only Windows daemon status: write-newline"
                }
                Self::SyncTemporary => {
                    "failed to write owner-only Windows daemon status: sync-temporary-file"
                }
                Self::ConvertDescriptor => {
                    "failed to write owner-only Windows daemon status: convert-security-descriptor"
                }
                Self::ApplySecurity => {
                    "failed to write owner-only Windows daemon status: apply-owner-only-security"
                }
                Self::OpenToken => {
                    "failed to write owner-only Windows daemon status: open-process-token"
                }
                Self::ReadToken => {
                    "failed to write owner-only Windows daemon status: read-process-token"
                }
                Self::ReplaceStatus => {
                    "failed to write owner-only Windows daemon status: replace-status-file"
                }
            },
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_preserve_os_errors_and_expose_only_fixed_operation_labels() {
        let stages = [
            (
                StatusWriteStage::CreateTemporary,
                "failed to write owner-only Windows daemon status: create-temporary-file",
            ),
            (
                StatusWriteStage::WriteContents,
                "failed to write owner-only Windows daemon status: write-contents",
            ),
            (
                StatusWriteStage::WriteNewline,
                "failed to write owner-only Windows daemon status: write-newline",
            ),
            (
                StatusWriteStage::SyncTemporary,
                "failed to write owner-only Windows daemon status: sync-temporary-file",
            ),
            (
                StatusWriteStage::ConvertDescriptor,
                "failed to write owner-only Windows daemon status: convert-security-descriptor",
            ),
            (
                StatusWriteStage::ApplySecurity,
                "failed to write owner-only Windows daemon status: apply-owner-only-security",
            ),
            (
                StatusWriteStage::OpenToken,
                "failed to write owner-only Windows daemon status: open-process-token",
            ),
            (
                StatusWriteStage::ReadToken,
                "failed to write owner-only Windows daemon status: read-process-token",
            ),
            (
                StatusWriteStage::ReplaceStatus,
                "failed to write owner-only Windows daemon status: replace-status-file",
            ),
        ];
        for (stage, expected) in stages {
            for code in [2, 3, 5, 32, 1307, 1314, 9999] {
                let error = stage.io_error(std::io::Error::from_raw_os_error(code));
                let ClientError::Io { operation, source } = error else {
                    panic!("writer error must retain its I/O variant");
                };
                assert_eq!(operation, expected);
                assert_eq!(source.raw_os_error(), Some(code));
            }
        }
    }
}
