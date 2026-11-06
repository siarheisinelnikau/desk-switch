pub mod hardware;
pub mod net;

use crate::{config, lifecycle::InputStatus, video::Inputs};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PORT: u16 = 45878;
pub const INPUT_PORT: u16 = 45876;
pub const AUDIO_PORT: u16 = 45877;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Hardware {
    pub known: bool,
    pub keyboard: bool,
    pub mouse: bool,
    pub audio_score: u8,
    pub audio_name: String,
}
impl Hardware {
    pub fn input(&self) -> bool {
        self.keyboard && self.mouse
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Monitor {
    pub enabled: bool,
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub input: Option<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Selection {
    #[default]
    Auto,
    Computer(String),
}

#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Revision {
    pub counter: u64,
    pub author: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Shared {
    pub revision: Revision,
    pub input: Selection,
    pub audio: Selection,
    pub audio_enabled: bool,
    pub volume: u8,
    pub muted: bool,
    pub edge: bool,
    pub purple_on_left: bool,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            revision: Revision::default(),
            input: Selection::Auto,
            audio: Selection::Auto,
            audio_enabled: true,
            volume: 50,
            muted: false,
            edge: false,
            purple_on_left: true,
        }
    }
}
impl Shared {
    pub fn changed(&mut self, author: &str) {
        self.revision.counter = self.revision.counter.saturating_add(1);
        self.revision.author = author.into();
    }
    pub fn merge(&mut self, other: &Self) -> bool {
        if other.revision > self.revision {
            *self = other.clone();
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    pub shared: Shared,
    pub monitor: Monitor,
    pub peer_name: String,
    pub last_address: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Node {
    pub key: String,
    pub name: String,
    pub hardware: Hardware,
    pub monitor: Monitor,
    pub shared: Shared,
    pub input: Option<InputStatus>,
    pub audio_playing: bool,
    pub audio_error: Option<String>,
    pub input_error: Option<String>,
    pub ack: String,
}
impl Node {
    pub fn validate(&self, own: &str, peer: &str) -> Result<()> {
        ensure!(
            self.key == peer && own != peer,
            "unexpected computer identity"
        );
        ensure!(
            self.name.len() <= 256 && self.name.chars().all(|c| !c.is_control()),
            "invalid computer name"
        );
        ensure!(
            self.hardware.audio_score <= 100 && self.hardware.audio_name.len() <= 512,
            "invalid audio information"
        );
        ensure!(
            self.monitor.id.len() <= 1024
                && self.monitor.name.len() <= 256
                && self.monitor.fingerprint.len() <= 128,
            "invalid monitor information"
        );
        ensure!(self.monitor.input != Some(0), "invalid monitor input");
        ensure!(self.shared.volume <= 100, "invalid volume");
        ensure!(
            self.shared.revision.counter < u64::MAX
                && (self.shared.revision.author.is_empty()
                    || [own, peer].contains(&self.shared.revision.author.as_str())),
            "invalid settings revision"
        );
        for choice in [&self.shared.input, &self.shared.audio] {
            if let Selection::Computer(key) = choice {
                ensure!(
                    [own, peer].contains(&key.as_str()),
                    "unknown computer in settings"
                );
            }
        }
        ensure!(
            self.ack.is_empty()
                || (self.ack.len() == 64 && self.ack.bytes().all(|c| c.is_ascii_hexdigit())),
            "invalid synchronization acknowledgement"
        );
        for error in [&self.audio_error, &self.input_error].into_iter().flatten() {
            ensure!(error.len() <= 2048, "status is too long");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Plan {
    pub purple: String,
    pub green: String,
    pub input_source: Option<String>,
    pub audio_output: Option<String>,
    pub monitor: Option<Inputs>,
    pub edge: bool,
    pub purple_on_left: bool,
}
impl Plan {
    pub fn build(a: &Node, b: &Node) -> Self {
        let (purple, green) = if a.key < b.key { (a, b) } else { (b, a) };
        let shared = if a.shared.revision >= b.shared.revision {
            &a.shared
        } else {
            &b.shared
        };
        let input_source = match &shared.input {
            Selection::Computer(key) => [a, b]
                .into_iter()
                .find(|n| &n.key == key && n.hardware.input()),
            Selection::Auto => match (a.hardware.input(), b.hardware.input()) {
                (true, false) => Some(a),
                (false, true) => Some(b),
                _ => None,
            },
        }
        .map(|n| n.key.clone());
        let audio_output = if !shared.audio_enabled {
            None
        } else {
            match &shared.audio {
                Selection::Computer(key) => [a, b]
                    .into_iter()
                    .find(|n| &n.key == key && n.hardware.audio_score > 0),
                Selection::Auto => [purple, green]
                    .into_iter()
                    .filter(|n| n.hardware.audio_score > 0)
                    .max_by_key(|n| {
                        (
                            n.hardware.audio_score,
                            input_source.as_ref() == Some(&n.key),
                            n.key == purple.key,
                        )
                    }),
            }
            .map(|n| n.key.clone())
        };
        let monitor = input_source.as_ref().and_then(|source| {
            let (host, peer) = if source == &a.key {
                (&a.monitor, &b.monitor)
            } else {
                (&b.monitor, &a.monitor)
            };
            (host.enabled
                && peer.enabled
                && !host.fingerprint.is_empty()
                && host.fingerprint == peer.fingerprint)
                .then_some((host.input?, peer.input?))
                .filter(|(a, b)| a != b)
                .map(|(host, peer)| Inputs { host, peer })
        });
        Self {
            purple: purple.key.clone(),
            green: green.key.clone(),
            input_source,
            audio_output,
            monitor,
            edge: shared.edge,
            purple_on_left: shared.purple_on_left,
        }
    }
    pub fn digest(&self, a: &Node, b: &Node) -> String {
        let (a, b) = if a.key < b.key { (a, b) } else { (b, a) };
        // Runtime status/volume never reconfigure services. Both sides acknowledge
        // the same physical devices, local monitor mappings and routing decision.
        let bytes =
            serde_json::to_vec(&(self, &a.hardware, &b.hardware, &a.monitor, &b.monitor)).unwrap();
        config::hex(&Sha256::digest(bytes))
    }
}

pub fn pairing_code(handshake: &[u8]) -> String {
    let hash = Sha256::digest(handshake);
    format!(
        "{:03} {:03}",
        u16::from_be_bytes([hash[0], hash[1]]) % 1000,
        u16::from_be_bytes([hash[2], hash[3]]) % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn node(key: &str) -> Node {
        Node {
            key: key.into(),
            name: key.into(),
            hardware: Hardware::default(),
            monitor: Monitor::default(),
            shared: Shared::default(),
            input: None,
            audio_playing: false,
            audio_error: None,
            input_error: None,
            ack: String::new(),
        }
    }
    #[test]
    fn both_views_agree_on_colours_and_hotplug_roles() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        a.hardware.audio_score = 20;
        b.hardware.audio_score = 40;
        let first = Plan::build(&a, &b);
        assert_eq!(first, Plan::build(&b, &a));
        assert_eq!(first.digest(&a, &b), first.digest(&b, &a));
        assert_eq!(first.input_source.as_deref(), Some("a"));
        assert_eq!(first.audio_output.as_deref(), Some("b"));
        b.hardware.keyboard = true;
        b.hardware.mouse = true;
        assert!(Plan::build(&a, &b).input_source.is_none());
        a.hardware.keyboard = false;
        a.hardware.mouse = false;
        assert_eq!(Plan::build(&a, &b).input_source.as_deref(), Some("b"));
        assert_ne!(first.digest(&a, &b), Plan::build(&a, &b).digest(&a, &b));
    }
    #[test]
    fn one_setting_change_converges_including_concurrent_edits() {
        let mut a = Shared::default();
        let mut b = a.clone();
        a.audio = Selection::Computer("a".into());
        a.changed("a");
        b.input = Selection::Computer("b".into());
        b.changed("b");
        let original = a.clone();
        a.merge(&b);
        b.merge(&original);
        assert_eq!(a, b);
        a.volume = 80;
        a.changed("a");
        b.merge(&a);
        assert_eq!(b.volume, 80);
    }
    #[test]
    fn monitor_mapping_comes_from_each_pcs_own_active_input() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        a.monitor = Monitor {
            enabled: true,
            id: "local-A".into(),
            fingerprint: "same display".into(),
            input: Some(17),
            ..Default::default()
        };
        b.monitor = Monitor {
            enabled: true,
            id: "local-B".into(),
            fingerprint: "same display".into(),
            input: Some(15),
            ..Default::default()
        };
        assert_eq!(
            Plan::build(&a, &b).monitor,
            Some(Inputs { host: 17, peer: 15 })
        );
        b.shared.input = Selection::Computer("b".into());
        b.shared.changed("b");
        b.hardware.keyboard = true;
        b.hardware.mouse = true;
        assert_eq!(
            Plan::build(&a, &b).monitor,
            Some(Inputs { host: 15, peer: 17 })
        );
        b.monitor.fingerprint = "different display".into();
        assert!(Plan::build(&a, &b).monitor.is_none());
    }
}
