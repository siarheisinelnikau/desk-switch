//! Model identification and standard MCCS input packets for the LG 27GK750F.
//! This model uses VCP 0x60, not the private 0xf4 dialect of some other LGs.
use anyhow::{Result, bail};

pub fn input(edid: &[u8], logical: u16) -> Result<Option<u8>> {
    // Use the NVIDIA transport only for the identified model, never for every
    // LG or merely because a monitor advertises a manufacturer VCP feature.
    if edid.len() < 128
        || !edid.starts_with(&[0, 255, 255, 255, 255, 255, 255, 0])
        || edid[8..10] != [0x1e, 0x6d]
    {
        return Ok(None);
    }
    let model = edid[54..126]
        .as_chunks::<18>()
        .0
        .iter()
        .find_map(|descriptor| {
            (descriptor[..5] == [0, 0, 0, 0xfc, 0]).then(|| {
                String::from_utf8_lossy(&descriptor[5..])
                    .trim_matches(['\0', '\n', '\r', ' '])
                    .to_owned()
            })
        });
    if model.as_deref() != Some("27GK750F") {
        return Ok(None);
    }
    Ok(Some(match logical {
        0x11 | 0x12 | 0x0f => logical as u8,
        other => bail!(
            "LG 27GK750F does not have configured input 0x{other:04x}; choose HDMI 1, HDMI 2 or DisplayPort"
        ),
    }))
}

#[cfg(any(windows, test))]
pub fn packet(value: u8) -> [u8; 6] {
    let mut packet = [0x84, 0x03, 0x60, 0, value, 0];
    packet[5] = packet[..5].iter().fold(0x6e ^ 0x51, |sum, byte| sum ^ byte);
    packet
}

#[cfg(test)]
pub fn fixture() -> Vec<u8> {
    let mut edid = vec![0; 128];
    edid[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
    edid[8..10].copy_from_slice(&[0x1e, 0x6d]);
    edid[54..59].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
    edid[59..67].copy_from_slice(b"27GK750F");
    edid
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identified_lg_uses_standard_mccs_packets_in_both_directions() {
        let mut edid = fixture();
        assert_eq!(input(&edid, 0x11).unwrap(), Some(0x11));
        assert_eq!(input(&edid, 0x12).unwrap(), Some(0x12));
        assert_eq!(input(&edid, 0x0f).unwrap(), Some(0x0f));
        assert!(input(&edid, 0x10).is_err());
        assert_eq!(packet(0x11), [0x84, 0x03, 0x60, 0x00, 0x11, 0xc9]);
        for value in [0x11, 0x12, 0x0f] {
            let bytes = packet(value);
            assert_eq!(&bytes[..5], &[0x84, 0x03, 0x60, 0, value]);
            assert_eq!(bytes.iter().fold(0x6e ^ 0x51, |sum, byte| sum ^ byte), 0);
        }
        edid[59] = b'X';
        assert_eq!(input(&edid, 0x11).unwrap(), None);
        edid[59] = b'2';
        edid[8] = 0x10;
        assert_eq!(input(&edid, 0x11).unwrap(), None);
        assert_eq!(input(&[], 0x11).unwrap(), None);
    }
}
