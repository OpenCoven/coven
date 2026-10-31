//! Private libuv stdio metadata. Native pipe behavior needs separate Windows proof.
use crate::Failure;

/// Decode the exact private four-pipe libuv startup contract, not arbitrary CRT data.
pub fn decode_node_lease(bytes: &[u8], handle_bytes: usize) -> Result<u64, Failure> {
    Ok(decode_node_pipes(bytes, handle_bytes)?[3])
}

/// All four descriptors in stdin/stdout/stderr/control order. Native callers
/// must duplicate noninheritable handles and validate their byte-pipe kind.
pub fn decode_node_pipes(bytes: &[u8], handle_bytes: usize) -> Result<[u64; 4], Failure> {
    if !matches!(handle_bytes, 4 | 8)
        || bytes.len() != 8 + 4 * handle_bytes
        || bytes[..4] != 4u32.to_le_bytes()
        || bytes[4..8] != [9; 4]
    {
        return Err(Failure::InvalidLaunch);
    }
    let maximum = if handle_bytes == 4 {
        u32::MAX as u64
    } else {
        u64::MAX
    };
    let mut handles = [0; 4];
    for fd in 0..4 {
        let mut encoded = [0; 8];
        let start = 8 + fd * handle_bytes;
        encoded[..handle_bytes].copy_from_slice(&bytes[start..start + handle_bytes]);
        let handle = u64::from_le_bytes(encoded);
        // Refuse null and the documented process/thread/token pseudo handles.
        // Native pipe validation follows before this becomes a usable lease.
        if handle == 0 || handle >= maximum - 5 || handles[..fd].contains(&handle) {
            return Err(Failure::InvalidLaunch);
        }
        handles[fd] = handle;
    }
    Ok(handles)
}

#[cfg(windows)]
pub mod native;

#[cfg(test)]
mod tests {
    use super::*;

    fn table(width: usize) -> Vec<u8> {
        let mut bytes = 4u32.to_le_bytes().to_vec();
        bytes.extend([9; 4]);
        for handle in [16u64, 20, 24, 28] {
            bytes.extend_from_slice(&handle.to_le_bytes()[..width]);
        }
        bytes
    }
    #[test]
    fn decodes_four_distinct_pipe_descriptors_for_both_handle_widths() {
        for width in [4, 8] {
            assert_eq!(decode_node_lease(&table(width), width), Ok(28));
            assert_eq!(
                decode_node_pipes(&table(width), width),
                Ok([16, 20, 24, 28])
            );
        }
    }
    #[test]
    fn refuses_all_truncated_or_extended_tables_and_wrong_counts() {
        for width in [4, 8] {
            let bytes = table(width);
            for end in 0..bytes.len() {
                assert_eq!(
                    decode_node_lease(&bytes[..end], width),
                    Err(Failure::InvalidLaunch)
                );
            }
            let mut extended = bytes.clone();
            extended.push(0);
            assert_eq!(
                decode_node_lease(&extended, width),
                Err(Failure::InvalidLaunch)
            );
            for count in [0u32, 3, 5, 255, u32::MAX] {
                let mut bad = bytes.clone();
                bad[..4].copy_from_slice(&count.to_le_bytes());
                assert_eq!(decode_node_lease(&bad, width), Err(Failure::InvalidLaunch));
            }
        }
        for width in [0, 1, 2, 3, 5, 16, usize::MAX] {
            assert_eq!(
                decode_node_lease(&table(8), width),
                Err(Failure::InvalidLaunch)
            );
        }
    }
    #[test]
    fn refuses_missing_pipe_flags_null_pseudo_and_aliased_handles() {
        for width in [4, 8] {
            for fd in 0..4 {
                for flags in [0, 1, 8, 0x19, 0x49, 0x89, 255] {
                    let mut bad = table(width);
                    bad[4 + fd] = flags;
                    assert_eq!(decode_node_lease(&bad, width), Err(Failure::InvalidLaunch));
                }
                for handle in [
                    0u64,
                    u64::MAX,
                    u64::MAX - 1,
                    u64::MAX - 2,
                    u64::MAX - 3,
                    u64::MAX - 4,
                    u64::MAX - 5,
                ] {
                    let mut bad = table(width);
                    let start = 8 + fd * width;
                    bad[start..start + width].copy_from_slice(&handle.to_le_bytes()[..width]);
                    assert_eq!(decode_node_lease(&bad, width), Err(Failure::InvalidLaunch));
                }
            }
            for fd in 0..3 {
                let mut bad = table(width);
                let start = 8 + fd * width;
                bad[start..start + width].copy_from_slice(&28u64.to_le_bytes()[..width]);
                assert_eq!(decode_node_lease(&bad, width), Err(Failure::InvalidLaunch));
            }
        }
    }
}
