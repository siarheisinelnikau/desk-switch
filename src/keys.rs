pub fn scan_code(scan: u32, extended: bool, vk: u32) -> Option<u16> {
    if vk == 0x13 {
        return Some(119);
    } // Pause has an E1 prefix.
    if vk == 0x2c {
        return Some(99);
    } // Print Screen
    if !extended {
        return match scan {
            1..=83 | 86..=88 => Some(scan as u16),
            0x70 => Some(93),
            0x73 => Some(89),
            0x79 => Some(92),
            0x7b => Some(94),
            0x7d => Some(124),
            _ => None,
        };
    }
    Some(match scan {
        0x10 => 165,
        0x19 => 163,
        0x1c => 96,
        0x1d => 97,
        0x20 => 113,
        0x21 => 140,
        0x22 => 164,
        0x24 => 166,
        0x2e => 114,
        0x30 => 115,
        0x32 => 172,
        0x35 => 98,
        0x38 => 100,
        0x47 => 102,
        0x48 => 103,
        0x49 => 104,
        0x4b => 105,
        0x4d => 106,
        0x4f => 107,
        0x50 => 108,
        0x51 => 109,
        0x52 => 110,
        0x53 => 111,
        0x5b => 125,
        0x5c => 126,
        0x5d => 127,
        0x65 => 217,
        0x66 => 156,
        0x67 => 173,
        0x68 => 128,
        0x69 => 159,
        0x6a => 158,
        _ => return None,
    })
}

pub fn windows_key(code: u16) -> Option<(u16, bool, bool)> {
    match code {
        119 => return Some((0x13, false, true)),
        99 => return Some((0x2c, false, true)),
        _ => {}
    }
    for extended in [false, true] {
        for scan in 1..=0x7f {
            if scan_code(scan, extended, 0) == Some(code) {
                return Some((scan as u16, extended, false));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_key_mapping_round_trips() {
        for code in [
            1, 30, 42, 54, 57, 87, 88, 96, 97, 100, 103, 105, 111, 113, 125,
        ] {
            let (scan, extended, virtual_key) = windows_key(code).unwrap();
            assert!(!virtual_key);
            assert_eq!(scan_code(scan.into(), extended, 0), Some(code));
        }
        assert_eq!(windows_key(119), Some((0x13, false, true)));
        assert!(windows_key(240).is_none());
    }
}
