use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

pub fn peer_address(text: &str) -> Result<SocketAddr> {
    let text = text.trim();
    ensure!(text.len() <= 128, "IP address is too long");
    let address = text
        .parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, super::PORT))
        .or_else(|_| text.parse::<SocketAddr>())
        .context("Enter an IP address, optionally followed by :port")?;
    let address = if let SocketAddr::V6(v6) = address {
        v6.ip()
            .to_ipv4_mapped()
            .map(|ip| SocketAddr::new(ip.into(), v6.port()))
            .unwrap_or(address)
    } else {
        address
    };
    ensure!(
        address.port() != 0
            && !address.ip().is_unspecified()
            && !address.ip().is_multicast()
            && address.ip() != IpAddr::V4(std::net::Ipv4Addr::BROADCAST),
        "Enter the other computer's unicast IP address"
    );
    Ok(address)
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Choices {
    pub keyboard: Option<String>,
    pub mouse: Option<String>,
    pub speaker: Option<String>,
    pub microphone: Option<String>,
    pub microphone_bridge: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Inventory {
    pub displays: Vec<Display>,
    pub keyboards: Vec<crate::audio::AudioDevice>,
    pub mice: Vec<crate::audio::AudioDevice>,
    pub speakers: Vec<crate::audio::AudioDevice>,
    pub microphones: Vec<crate::audio::AudioDevice>,
    pub microphone_bridges: Vec<crate::audio::AudioDevice>,
    pub individual_input: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
}
impl Display {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 1024
                && self.name.len() <= 256
                && !self.id.chars().any(char::is_control)
                && !self.name.chars().any(char::is_control)
                && (self.fingerprint.is_empty()
                    || (self.fingerprint.len() == 64
                        && self
                            .fingerprint
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit()))),
            "Invalid display information"
        );
        Ok(())
    }
}

impl Inventory {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.displays.len() <= 32, "Too many displays");
        let mut display_ids = std::collections::BTreeSet::new();
        for display in &self.displays {
            display.validate()?;
            ensure!(
                display_ids.insert(&display.id),
                "Duplicate display information"
            );
        }
        for list in [
            &self.keyboards,
            &self.mice,
            &self.speakers,
            &self.microphones,
            &self.microphone_bridges,
        ] {
            ensure!(list.len() <= 32, "too many shared devices");
            let mut ids = std::collections::BTreeSet::new();
            for device in list {
                ensure!(
                    !device.id.is_empty()
                        && device.id.len() <= 2048
                        && device.name.len() <= 256
                        && !device.id.chars().any(char::is_control)
                        && !device.name.chars().any(char::is_control)
                        && ids.insert(&device.id),
                    "invalid device information"
                );
            }
        }
        Ok(())
    }
}

pub fn selected<'a>(
    devices: &'a [crate::audio::AudioDevice],
    choice: Option<&str>,
) -> Option<&'a crate::audio::AudioDevice> {
    match choice {
        Some(id) => devices.iter().find(|device| device.id == id),
        None => devices
            .iter()
            .find(|device| device.default)
            .or_else(|| devices.first()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_inventory_accepts_legacy_reports_and_rejects_ambiguous_or_unbounded_displays() {
        let legacy: Inventory = serde_json::from_str("{\"individual_input\":true}").unwrap();
        assert!(legacy.displays.is_empty());
        let display = Display {
            id: "display-1".into(),
            name: "Desk monitor".into(),
            fingerprint: "ab".repeat(32),
        };
        let mut inventory = Inventory {
            displays: vec![display.clone()],
            ..Default::default()
        };
        inventory.validate().unwrap();
        inventory.displays.push(display.clone());
        assert!(inventory.validate().is_err());
        for invalid in [
            Display {
                id: "x".repeat(1025),
                ..display.clone()
            },
            Display {
                name: "line\nbreak".into(),
                ..display.clone()
            },
            Display {
                fingerprint: "q".repeat(64),
                ..display.clone()
            },
            Display {
                fingerprint: "a".repeat(63),
                ..display.clone()
            },
        ] {
            inventory.displays = vec![invalid];
            assert!(inventory.validate().is_err());
        }
        inventory.displays = (0..33)
            .map(|index| Display {
                id: format!("display-{index}"),
                ..display.clone()
            })
            .collect();
        assert!(inventory.validate().is_err());
    }
    #[test]
    fn manual_address_supports_routed_ipv4_ipv6_and_explicit_ports() {
        assert_eq!(
            peer_address("10.20.30.40").unwrap().to_string(),
            "10.20.30.40:45878"
        );
        assert_eq!(peer_address("10.20.30.40:1234").unwrap().port(), 1234);
        assert_eq!(
            peer_address("::ffff:10.20.30.40").unwrap().to_string(),
            "10.20.30.40:45878"
        );
        assert_eq!(peer_address("fd00::1").unwrap().port(), super::super::PORT);
        assert_eq!(peer_address("[fd00::1]:1234").unwrap().port(), 1234);
        for value in [
            "",
            "host.example",
            "0.0.0.0",
            "::",
            "224.0.0.1",
            "255.255.255.255",
            "10.0.0.1:0",
        ] {
            assert!(peer_address(value).is_err(), "{value}");
        }
    }

    #[test]
    fn an_unavailable_manual_choice_never_falls_back_to_another_device() {
        let devices = vec![crate::audio::AudioDevice {
            id: "webcam".into(),
            name: "Webcam microphone".into(),
            default: true,
        }];
        assert_eq!(selected(&devices, None).unwrap().id, "webcam");
        assert!(selected(&devices, Some("usb-headset")).is_none());
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Binding {
    pub computer: String,
    pub device: String,
}
