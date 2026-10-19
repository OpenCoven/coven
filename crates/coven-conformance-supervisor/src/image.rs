//! Bounded image metadata and identity inputs; not a complete Windows loader.
use crate::Failure;
use std::io::{Read, Seek};

pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u32 = 512 * 1024 * 1024;
pub const MAX_PATH_UNITS: usize = 8192;
pub const MAX_DIRECTORY_HANDLES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X64,
    Arm64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeInfo {
    pub architecture: Architecture,
    pub image_bytes: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub struct PathLayout {
    pub root_end: usize,
    pub component_ends: Vec<usize>,
}

pub fn read_pe(reader: &mut (impl Read + Seek), length: u64) -> Result<PeInfo, Failure> {
    use std::io::SeekFrom;
    if !(64..=MAX_FILE_BYTES).contains(&length) {
        return Err(Failure::InvalidLaunch);
    }
    let mut dos = [0; 64];
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| Failure::InvalidLaunch)?;
    reader
        .read_exact(&mut dos)
        .map_err(|_| Failure::InvalidLaunch)?;
    if &dos[..2] != b"MZ" {
        return Err(Failure::InvalidLaunch);
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if offset < 64 || offset > length - 24 {
        return Err(Failure::InvalidLaunch);
    }
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|_| Failure::InvalidLaunch)?;
    let mut coff = [0; 24];
    reader
        .read_exact(&mut coff)
        .map_err(|_| Failure::InvalidLaunch)?;
    let architecture = match u16::from_le_bytes([coff[4], coff[5]]) {
        0x8664 => Architecture::X64,
        0xaa64 => Architecture::Arm64,
        _ => return Err(Failure::InvalidLaunch),
    };
    let sections = u16::from_le_bytes([coff[6], coff[7]]) as u64;
    let optional_size = u16::from_le_bytes([coff[20], coff[21]]) as u64;
    let flags = u16::from_le_bytes([coff[22], coff[23]]);
    if &coff[..4] != b"PE\0\0"
        || !(1..=96).contains(&sections)
        || optional_size < 112
        || offset + 24 + optional_size + sections * 40 > length
        || flags & 2 == 0
        || flags & 0x2000 != 0
    {
        return Err(Failure::InvalidLaunch);
    }
    let mut optional = [0; 112];
    reader
        .read_exact(&mut optional)
        .map_err(|_| Failure::InvalidLaunch)?;
    let image_bytes = u32::from_le_bytes(optional[56..60].try_into().unwrap());
    if optional[..2] != 0x20bu16.to_le_bytes() || image_bytes == 0 || image_bytes > MAX_IMAGE_BYTES
    {
        return Err(Failure::InvalidLaunch);
    }
    Ok(PeInfo {
        architecture,
        image_bytes,
    })
}

fn ascii_word(units: &[u16], word: &[u8]) -> bool {
    units.len() == word.len()
        && units.iter().zip(word).all(|(&a, &b)| {
            let upper = if (97..=122).contains(&a) { a - 32 } else { a };
            upper == b as u16
        })
}
fn valid_component(part: &[u16]) -> bool {
    if part.is_empty()
        || part.len() > 255
        || matches!(part.last(), Some(32 | 46))
        || part
            .iter()
            .any(|&u| matches!(u, 0..=31 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 124))
    {
        return false;
    }
    let stem_end = part.iter().position(|&u| u == 46).unwrap_or(part.len());
    let mut stem = &part[..stem_end];
    while stem.last() == Some(&32) {
        stem = &stem[..stem.len() - 1];
    }
    if [
        b"CON".as_slice(),
        b"PRN",
        b"AUX",
        b"NUL",
        b"CONIN$",
        b"CONOUT$",
    ]
    .iter()
    .any(|word| ascii_word(stem, word))
    {
        return false;
    }
    !(stem.len() == 4
        && (ascii_word(&stem[..3], b"COM") || ascii_word(&stem[..3], b"LPT"))
        && matches!(stem[3], 49..=57 | 0xb9 | 0xb2 | 0xb3))
}

pub fn windows_path(units: &[u16]) -> Result<PathLayout, Failure> {
    if units.len() > MAX_PATH_UNITS {
        return Err(Failure::InvalidLaunch);
    }
    let drive = |value: u16| (65..=90).contains(&value) || (97..=122).contains(&value);
    let root_end = if units.len() >= 3 && drive(units[0]) && units[1..3] == [58, 92] {
        3
    } else if units.len() >= 7
        && units[..4] == [92, 92, 63, 92]
        && drive(units[4])
        && units[5..7] == [58, 92]
    {
        7
    } else {
        return Err(Failure::InvalidLaunch);
    };
    let mut component_ends = Vec::new();
    let mut start = root_end;
    loop {
        let end = units[start..]
            .iter()
            .position(|&u| u == 92)
            .map_or(units.len(), |n| start + n);
        if component_ends.len() >= MAX_DIRECTORY_HANDLES || !valid_component(&units[start..end]) {
            return Err(Failure::InvalidLaunch);
        }
        component_ends.push(end);
        if end == units.len() {
            let name = &units[start..end];
            if name.len() < 4
                || name[name.len() - 4] != 46
                || !ascii_word(&name[name.len() - 3..], b"EXE")
            {
                return Err(Failure::InvalidLaunch);
            }
            break;
        }
        start = end + 1;
    }
    Ok(PathLayout {
        root_end,
        component_ends,
    })
}

pub fn stream_sha256(reader: &mut impl Read, length: u64) -> Result<String, Failure> {
    use sha2::{Digest, Sha256};
    if length > MAX_FILE_BYTES {
        return Err(Failure::InvalidLaunch);
    }
    let mut reader = reader.take(length + 1);
    let mut buffer = [0; 65536];
    let mut digest = Sha256::new();
    let mut read = 0;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| Failure::InvalidLaunch)?;
        if count == 0 {
            break;
        }
        read += count as u64;
        if read > length {
            return Err(Failure::InvalidLaunch);
        }
        digest.update(&buffer[..count]);
    }
    if read != length {
        return Err(Failure::InvalidLaunch);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest.finalize().iter().copied() {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(output)
}

#[cfg(any(windows, test))]
pub(crate) fn physical_path(
    original: &[u16],
    native_root: &[u16],
) -> Result<(Vec<u16>, PathLayout), Failure> {
    let layout = windows_path(original)?;
    let prefix = b"\\DEVICE\\HARDDISKVOLUME";
    let root = native_root.strip_suffix(&[92]).unwrap_or(native_root);
    if root.len() <= prefix.len()
        || !ascii_word(&root[..prefix.len()], prefix)
        || root.len() - prefix.len() > 20
        || !root[prefix.len()..].iter().all(|&u| (48..=57).contains(&u))
    {
        return Err(Failure::InvalidLaunch);
    }
    let mut program: Vec<u16> = r"\\?\GLOBALROOT".encode_utf16().collect();
    program.extend_from_slice(root);
    program.push(92);
    let root_end = program.len();
    program.extend_from_slice(&original[layout.root_end..]);
    if program.len() > MAX_PATH_UNITS {
        return Err(Failure::InvalidLaunch);
    }
    let component_ends = layout
        .component_ends
        .into_iter()
        .map(|end| root_end + end - layout.root_end)
        .collect();
    Ok((
        program,
        PathLayout {
            root_end,
            component_ends,
        },
    ))
}

#[cfg(windows)]
pub mod native;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};
    fn pe(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0; 512];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&128u32.to_le_bytes());
        bytes[128..132].copy_from_slice(b"PE\0\0");
        bytes[132..134].copy_from_slice(&machine.to_le_bytes());
        bytes[134..136].copy_from_slice(&1u16.to_le_bytes());
        bytes[148..150].copy_from_slice(&112u16.to_le_bytes());
        bytes[150..152].copy_from_slice(&2u16.to_le_bytes());
        bytes[152..154].copy_from_slice(&0x20bu16.to_le_bytes());
        bytes[208..212].copy_from_slice(&4096u32.to_le_bytes());
        bytes
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }
    fn refused(bytes: &[u8]) {
        assert_eq!(
            read_pe(&mut Cursor::new(bytes), bytes.len() as u64),
            Err(Failure::InvalidLaunch)
        );
    }
    #[test]
    fn reads_x64_and_arm64_metadata_with_complete_bounded_tables() {
        for (machine, arch) in [(0x8664, Architecture::X64), (0xaa64, Architecture::Arm64)] {
            let bytes = pe(machine);
            let mut reader = Cursor::new(bytes);
            assert_eq!(
                read_pe(&mut reader, 512),
                Ok(PeInfo {
                    architecture: arch,
                    image_bytes: 4096
                })
            );
            assert_eq!(reader.position(), 264);
        }
    }
    #[test]
    fn rejects_all_truncated_prefixes_and_invalid_headers() {
        let bytes = pe(0x8664);
        for end in 0..304 {
            refused(&bytes[..end]);
        }
        for (offset, part) in [
            (0, b"NO".as_slice()),
            (60, &0u32.to_le_bytes()),
            (60, &u32::MAX.to_le_bytes()),
            (128, b"NOPE"),
            (132, &0x14cu16.to_le_bytes()),
            (134, &0u16.to_le_bytes()),
            (134, &97u16.to_le_bytes()),
            (148, &111u16.to_le_bytes()),
            (148, &u16::MAX.to_le_bytes()),
            (150, &0u16.to_le_bytes()),
            (150, &0x2002u16.to_le_bytes()),
            (152, &0x10bu16.to_le_bytes()),
            (208, &0u32.to_le_bytes()),
            (208, &(MAX_IMAGE_BYTES + 1).to_le_bytes()),
        ] {
            let mut bad = bytes.clone();
            bad[offset..offset + part.len()].copy_from_slice(part);
            refused(&bad);
        }
        assert_eq!(
            read_pe(&mut Cursor::new(&bytes), MAX_FILE_BYTES + 1),
            Err(Failure::InvalidLaunch)
        );
    }
    #[test]
    fn permits_exact_section_and_image_bounds() {
        let mut bytes = pe(0x8664);
        bytes.resize(264 + 96 * 40, 0);
        bytes[134..136].copy_from_slice(&96u16.to_le_bytes());
        bytes[208..212].copy_from_slice(&MAX_IMAGE_BYTES.to_le_bytes());
        assert!(read_pe(&mut Cursor::new(&bytes), bytes.len() as u64).is_ok());
        refused(&bytes[..bytes.len() - 1]);
    }
    #[test]
    fn preserves_drive_paths_and_opaque_utf16_without_aliases() {
        for (path, root) in [
            (r"C:\private\subject.exe", 3),
            (r"\\?\D:\private\SUBJECT.EXE", 7),
        ] {
            let units = wide(path);
            let layout = windows_path(&units).unwrap();
            assert_eq!(layout.root_end, root);
            assert_eq!(layout.component_ends.last(), Some(&units.len()));
            assert_eq!(layout.component_ends.len(), 2);
        }
        let mut opaque = wide(r"C:\");
        opaque.push(0xd800);
        opaque.extend(wide(r"\subject.exe"));
        assert!(windows_path(&opaque).is_ok());
    }
    #[test]
    fn rejects_ambiguous_device_remote_relative_and_stream_names() {
        for path in [
            r"subject.exe",
            r"C:subject.exe",
            r"\subject.exe",
            r"\\server\share\subject.exe",
            r"\\.\C:\subject.exe",
            r"\\?\UNC\server\share\subject.exe",
            r"C:/private/subject.exe",
            r"C:\private\\subject.exe",
            r"C:\.\subject.exe",
            r"C:\..\subject.exe",
            r"C:\private.\subject.exe",
            r"C:\private \subject.exe",
            r"C:\private\subject.exe:stream",
            r"C:\private\subject",
            r"C:\private\subject.exe.",
            r"C:\private\subject.exe ",
            r"C:\private\subject.exe\",
            r"1:\subject.exe",
            r"C:\pri?vate\subject.exe",
            r"C:\pri|vate\subject.exe",
        ] {
            assert_eq!(
                windows_path(&wide(path)),
                Err(Failure::InvalidLaunch),
                "invalid fixture {path}"
            );
        }
        for unit in [0, 1, 31, 34, 42, 60, 62] {
            let mut units = wide(r"C:\private\subject.exe");
            units[4] = unit;
            assert_eq!(windows_path(&units), Err(Failure::InvalidLaunch));
        }
    }
    #[test]
    fn enforces_path_and_handle_counts_at_the_boundary() {
        let mut path = wide(r"C:\");
        for _ in 1..MAX_DIRECTORY_HANDLES {
            path.extend(wide("a\\"));
        }
        path.extend(wide("s.exe"));
        assert_eq!(
            windows_path(&path).unwrap().component_ends.len(),
            MAX_DIRECTORY_HANDLES
        );
        let mut too_deep = wide(r"C:\a\");
        too_deep.extend_from_slice(&path[3..]);
        assert_eq!(windows_path(&too_deep), Err(Failure::InvalidLaunch));
        let mut huge = wide(r"C:\");
        huge.extend(vec![97; MAX_PATH_UNITS]);
        huge.extend(wide(".exe"));
        assert_eq!(windows_path(&huge), Err(Failure::InvalidLaunch));
    }
    #[test]
    fn streams_exact_binary_bytes_and_refuses_length_mismatches() {
        assert_eq!(
            stream_sha256(&mut Cursor::new(b"abc"), 3).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            stream_sha256(&mut Cursor::new([]), 0).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        for length in [2, 4, MAX_FILE_BYTES + 1] {
            assert_eq!(
                stream_sha256(&mut Cursor::new(b"abc"), length),
                Err(Failure::InvalidLaunch)
            );
        }
        let bytes = vec![0xab; 65537];
        let a = stream_sha256(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(
            a,
            stream_sha256(&mut Cursor::new(&bytes[..65536]), 65536).unwrap()
        );
    }
    #[test]
    fn anchors_physical_program_to_kernel_volume_instead_of_dos_alias() {
        let root = wide(r"\Device\HarddiskVolume4\");
        let (program, layout) = physical_path(&wide(r"C:\private\subject.exe"), &root).unwrap();
        assert_eq!(
            program,
            wide(r"\\?\GLOBALROOT\Device\HarddiskVolume4\private\subject.exe")
        );
        assert_eq!(
            layout.root_end,
            wide(r"\\?\GLOBALROOT\Device\HarddiskVolume4\").len()
        );
        assert_eq!(layout.component_ends.last(), Some(&program.len()));
        assert_eq!(
            physical_path(&wide(r"D:\private\subject.exe"), &root)
                .unwrap()
                .0,
            program
        );
    }
    #[test]
    fn refuses_nonvolume_kernel_roots_and_physical_path_expansion_over_budget() {
        for root in [
            r"\Device\Mup\server\share",
            r"\Device\HarddiskVolume4\subdirectory",
            r"\??\C:\",
            r"\Device\HarddiskVolume",
            r"\Device\HarddiskVolume4\\",
        ] {
            assert_eq!(
                physical_path(&wide(r"C:\subject.exe"), &wide(root)),
                Err(Failure::InvalidLaunch)
            );
        }
        let mut maximum = wide(r"C:\");
        for _ in 0..31 {
            maximum.extend(vec![97; 255]);
            maximum.push(92);
        }
        maximum.extend(vec![97; 249]);
        maximum.extend(wide(".exe"));
        assert_eq!(maximum.len(), MAX_PATH_UNITS);
        assert!(windows_path(&maximum).is_ok());
        assert_eq!(
            physical_path(&maximum, &wide(r"\Device\HarddiskVolume4\")),
            Err(Failure::InvalidLaunch)
        );
    }
    #[test]
    fn refuses_reserved_device_names_including_extensions_and_superscripts() {
        for name in [
            "NUL.exe",
            "con.exe",
            "AUX.foo.exe",
            "COM1.exe",
            "LPT9.exe",
            "COM¹.exe",
            "LPT².exe",
            "CONIN$.exe",
            "CONOUT$.exe",
            "NUL .exe",
        ] {
            assert_eq!(
                windows_path(&wide(&format!("C:\\{name}"))),
                Err(Failure::InvalidLaunch)
            );
        }
        assert!(windows_path(&wide(r"C:\COM0.exe")).is_ok());
    }
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("PRIVATE path"))
        }
    }
    impl Seek for Broken {
        fn seek(&mut self, _: io::SeekFrom) -> io::Result<u64> {
            Err(io::Error::other("PRIVATE path"))
        }
    }
    #[test]
    fn minimizes_io_errors_and_bounds_declarations_before_reading() {
        assert_eq!(read_pe(&mut Broken, 512), Err(Failure::InvalidLaunch));
        assert_eq!(stream_sha256(&mut Broken, 1), Err(Failure::InvalidLaunch));
        let mut cursor = Cursor::new(b"abc");
        assert_eq!(
            stream_sha256(&mut cursor, MAX_FILE_BYTES + 1),
            Err(Failure::InvalidLaunch)
        );
        assert_eq!(cursor.position(), 0);
    }
}
