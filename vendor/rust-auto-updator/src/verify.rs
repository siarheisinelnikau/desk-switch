use crate::Architecture;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

pub fn checksum(text: &str, executable_name: &str) -> Result<String, String> {
    let mut found = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() >= 2 && fields[1].trim_start_matches('*') == executable_name {
            if fields.len() != 2
                || found.is_some()
                || fields[0].len() != 64
                || !fields[0].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("The release checksum file is invalid.".into());
            }
            found = Some(fields[0].to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| "The release checksum file does not include the executable.".into())
}

/// A repository-bound per-executable checksum asset may contain a single bare
/// digest. Multi-file manifests must use `checksum` and name the exact asset.
pub fn single_asset_checksum(text: &str, executable_name: &str) -> Result<String, String> {
    let value = text.trim();
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        checksum(text, executable_name)
    }
}

pub fn hash(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn verify_executable(
    path: &Path,
    expected: &str,
    size: u64,
    architecture: Architecture,
) -> Result<(), String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != size || hash(path)? != expected {
        return Err(
            "The downloaded executable failed SHA-256 verification. Nothing was installed.".into(),
        );
    }
    verify_pe(path, architecture)
}

/// Validate a complete portable PE executable, including section bounds and the
/// on-disk entry point. Both Windows GUI and console applications are accepted.
pub fn verify_pe(path: &Path, architecture: Architecture) -> Result<(), String> {
    verify_pe_subsystem(path, architecture, None)
}

pub fn verify_pe_subsystem(
    path: &Path,
    architecture: Architecture,
    expected_subsystem: Option<u16>,
) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as u64;
    if &header[..2] != b"MZ"
        || !(64..=1024 * 1024).contains(&offset)
        || offset > size.saturating_sub(24)
    {
        return Err("The download is not a Windows executable.".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut pe = [0u8; 24];
    file.read_exact(&mut pe).map_err(|e| e.to_string())?;
    if &pe[..4] != b"PE\0\0" || pe[4..6] != architecture.machine().to_le_bytes() {
        return Err(format!(
            "The download is not a Windows {architecture:?} executable."
        ));
    }
    let sections = u16::from_le_bytes([pe[6], pe[7]]) as usize;
    let optional_size = u16::from_le_bytes([pe[20], pe[21]]) as usize;
    let characteristics = u16::from_le_bytes([pe[22], pe[23]]);
    let (minimum_optional, magic) = match architecture {
        Architecture::X86 => (96, 0x010b),
        Architecture::X64 | Architecture::Arm64 => (112, 0x020b),
    };
    if !(1..=96).contains(&sections)
        || !(minimum_optional..=4096).contains(&optional_size)
        || characteristics & 0x0002 == 0
        || characteristics & 0x2000 != 0
    {
        return Err("The download is not a complete executable PE image.".into());
    }
    let mut optional = vec![0u8; optional_size];
    file.read_exact(&mut optional)
        .map_err(|e| format!("Truncated PE optional header: {e}"))?;
    let read_u32 = |bytes: &[u8], index: usize| {
        u32::from_le_bytes(bytes[index..index + 4].try_into().unwrap()) as u64
    };
    let entry = read_u32(&optional, 16);
    let image_size = read_u32(&optional, 56);
    let headers_size = read_u32(&optional, 60);
    let directory_count_offset = match architecture {
        Architecture::X86 => 92,
        _ => 108,
    };
    let directory_count = read_u32(&optional, directory_count_offset);
    if u16::from_le_bytes([optional[0], optional[1]]) != magic
        || ![2, 3].contains(&u16::from_le_bytes([optional[68], optional[69]]))
        || expected_subsystem
            .is_some_and(|expected| expected != u16::from_le_bytes([optional[68], optional[69]]))
        || entry == 0
        || entry >= image_size
        || headers_size > size
        || directory_count > (optional_size.saturating_sub(directory_count_offset + 4) / 8) as u64
        || offset + 24 + optional_size as u64 + sections as u64 * 40 > headers_size
    {
        return Err("The download has invalid PE executable headers.".into());
    }
    let mut table = vec![0u8; sections * 40];
    file.read_exact(&mut table)
        .map_err(|e| format!("Truncated PE section table: {e}"))?;
    let mut executable_entry = false;
    for section in table.as_chunks::<40>().0 {
        let virtual_size = read_u32(section, 8);
        let address = read_u32(section, 12);
        let raw_size = read_u32(section, 16);
        let raw = read_u32(section, 20);
        if raw_size > 0 && (raw < headers_size || raw + raw_size > size) {
            return Err("Executable section data is truncated or overlaps the headers.".into());
        }
        if address + virtual_size.max(raw_size) > image_size {
            return Err("PE section exceeds the image size.".into());
        }
        executable_entry |= entry >= address
            && entry < address + raw_size
            && read_u32(section, 36) & 0x20000000 != 0;
    }
    if !executable_entry {
        return Err("PE entry point is not backed by executable section data.".into());
    }
    Ok(())
}
