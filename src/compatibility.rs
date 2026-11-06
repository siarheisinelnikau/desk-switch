use anyhow::{Result, ensure};
use std::fmt;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PROTOCOL: u16 = 2;
pub const BUILD_INFO: &str = concat!(
    "DESK-SWITCH-BUILD:{\"app\":\"",
    env!("CARGO_PKG_VERSION"),
    "\",\"protocol\":2}"
);
const MAGIC: &[u8; 8] = b"DESKVER2";

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub app: String,
    pub protocol: u16,
}

impl Version {
    pub fn current() -> Self {
        Self {
            app: VERSION.into(),
            protocol: PROTOCOL,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure!(
            !self.app.is_empty() && self.app.len() <= 48 && self.app.is_ascii(),
            "invalid application version"
        );
        let mut data = Vec::from(*MAGIC);
        data.extend(self.protocol.to_be_bytes());
        data.push(self.app.len() as u8);
        data.extend(self.app.as_bytes());
        Ok(data)
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= 12 && data.starts_with(MAGIC),
            "peer does not provide a version; update the other PC"
        );
        let length = usize::from(data[10]);
        ensure!(
            (1..=48).contains(&length) && data.len() == 11 + length && data[11..].is_ascii(),
            "invalid peer version"
        );
        Ok(Self {
            app: std::str::from_utf8(&data[11..])?.into(),
            protocol: u16::from_be_bytes([data[8], data[9]]),
        })
    }

    pub fn require(&self, peer: Option<&Self>) -> Result<()> {
        if peer != Some(self) {
            return Err(Mismatch {
                local: self.clone(),
                peer: peer.cloned(),
            }
            .into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Mismatch {
    pub local: Version,
    pub peer: Option<Version>,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Несовместимые версии: этот ПК {} (протокол {}), другой ПК ",
            self.local.app, self.local.protocol
        )?;
        if let Some(peer) = &self.peer {
            write!(f, "{} (протокол {})", peer.app, peer.protocol)?;
        } else {
            write!(f, "не сообщает версию (старый клиент)")?;
        }
        write!(f, ". Обновите оба ПК до одной версии.")
    }
}
impl std::error::Error for Mismatch {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_exact_app_and_protocol_and_rejects_legacy_payloads() {
        let current = Version::current();
        assert_eq!(
            Version::decode(&current.encode().unwrap()).unwrap(),
            current
        );
        assert!(current.require(Some(&current)).is_ok());
        assert!(
            current
                .require(Some(&Version {
                    app: format!("{VERSION}-different"),
                    ..current.clone()
                }))
                .is_err()
        );
        assert!(
            current
                .require(Some(&Version {
                    protocol: PROTOCOL + 1,
                    ..current.clone()
                }))
                .is_err()
        );
        assert!(current.require(None).is_err());
        assert!(Version::decode(&[]).is_err());
        assert_eq!(
            serde_json::from_str::<Version>(BUILD_INFO.strip_prefix("DESK-SWITCH-BUILD:").unwrap())
                .unwrap(),
            current
        );
    }
}
