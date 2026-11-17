//! LG's private input-selection protocol. Logical port values stay MCCS values
//! in saved settings and on the wire; translation happens only at the device.
use anyhow::{Result, bail};

pub fn input(edid: &[u8], logical: u16) -> Result<Option<u8>> {
    // Use this workaround only for the identified model, never for every LG or
    // merely because a monitor happens to advertise a manufacturer VCP feature.
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
        0x11 => 0x90,
        0x12 => 0x91,
        0x0f => 0xd0,
        other => bail!(
            "LG 27GK750F does not have configured input 0x{other:04x}; choose HDMI 1, HDMI 2 or DisplayPort"
        ),
    }))
}

#[cfg(any(windows, test))]
pub fn packet(value: u8) -> [u8; 6] {
    let mut packet = [0x84, 0x03, 0xf4, 0, value, 0];
    packet[5] = packet[..5].iter().fold(0x6e ^ 0x50, |sum, byte| sum ^ byte);
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
    fn model_specific_input_packets_use_the_lg_source_address() {
        let mut edid = fixture();
        assert_eq!(input(&edid, 0x11).unwrap(), Some(0x90));
        assert_eq!(input(&edid, 0x12).unwrap(), Some(0x91));
        assert_eq!(input(&edid, 0x0f).unwrap(), Some(0xd0));
        assert!(input(&edid, 0x10).is_err());
        for value in [0x90, 0x91, 0xd0] {
            let bytes = packet(value);
            assert_eq!(&bytes[..5], &[0x84, 0x03, 0xf4, 0, value]);
            assert_eq!(bytes.iter().fold(0x6e ^ 0x50, |sum, byte| sum ^ byte), 0);
        }
        edid[59] = b'X';
        assert_eq!(input(&edid, 0x11).unwrap(), None);
        edid[59] = b'2';
        edid[8] = 0x10;
        assert_eq!(input(&edid, 0x11).unwrap(), None);
        assert_eq!(input(&[], 0x11).unwrap(), None);
    }
}
