pub mod hardware;
pub mod net;

use crate::{config, lifecycle::InputStatus, video::Inputs};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

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
    pub audio_levels: BTreeMap<String, AudioLevel>,
    pub keyboard_enabled: bool,
    pub mouse_enabled: bool,
    pub monitor_enabled: bool,
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
            audio_levels: BTreeMap::new(),
            keyboard_enabled: true,
            mouse_enabled: true,
            monitor_enabled: true,
            edge: false,
            purple_on_left: true,
        }
    }
}
impl Shared {
    pub fn audio_level(&self, source: &str) -> AudioLevel {
        self.audio_levels.get(source).copied().unwrap_or_default()
    }

    pub fn prepare_audio_levels(&mut self, a: &str, b: &str, output: Option<&str>) -> bool {
        let mut changed = false;
        for source in [a, b] {
            if !self.audio_levels.contains_key(source) {
                let level = if Some(source) == output {
                    AudioLevel::default()
                } else {
                    AudioLevel {
                        volume: self.volume,
                        muted: self.muted,
                    }
                };
                self.audio_levels.insert(source.into(), level);
                changed = true;
            }
        }
        changed
    }
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct AudioLevel {
    pub volume: u8,
    pub muted: bool,
}
impl Default for AudioLevel {
    fn default() -> Self {
        Self {
            volume: 100,
            muted: false,
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
    #[serde(default)]
    pub monitor_input: Option<u16>,
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
            self.shared.audio_levels.len() <= 2
                && self
                    .shared
                    .audio_levels
                    .iter()
                    .all(|(key, level)| [own, peer].contains(&key.as_str()) && level.volume <= 100),
            "invalid source audio levels"
        );
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
    pub keyboard_enabled: bool,
    pub mouse_enabled: bool,
    pub audio_output: Option<String>,
    pub monitor: Option<Inputs>,
    pub edge: bool,
    pub purple_on_left: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct InputDevices<'a> {
    pub keyboard: Option<&'a str>,
    pub mouse: Option<&'a str>,
}

impl Plan {
    pub fn input_in_use(a: &Node, b: &Node) -> bool {
        let source_key = Self::build(a, b).input_source;
        [(a, b), (b, a)].into_iter().any(|(source, target)| {
            source_key.as_ref() == Some(&source.key)
                && source.input == Some(InputStatus::Peer)
                && target.input == Some(InputStatus::Here)
                && source.hardware.input()
        })
    }

    pub fn can_disable(a: &Node, b: &Node, keyboard: bool) -> bool {
        ![(a, b), (b, a)].into_iter().any(|(source, target)| {
            (source.input == Some(InputStatus::Peer) || target.input == Some(InputStatus::Here))
                && source.hardware.input()
                && !(if keyboard {
                    target.hardware.keyboard
                } else {
                    target.hardware.mouse
                })
        })
    }

    pub fn shared_monitor(a: &Node, b: &Node) -> bool {
        a.monitor.enabled
            && b.monitor.enabled
            && !a.monitor.fingerprint.is_empty()
            && a.monitor.fingerprint == b.monitor.fingerprint
    }

    pub fn monitor_owner<'a>(a: &'a Node, b: &'a Node, synced: bool) -> Option<&'a str> {
        if !synced
            || !Self::shared_monitor(a, b)
            || !a
                .monitor
                .input
                .zip(b.monitor.input)
                .is_some_and(|(a, b)| a != 0 && b != 0 && a != b)
        {
            return None;
        }
        let observed = match (a.monitor_input, b.monitor_input) {
            (Some(a), Some(b)) if a != b => return None,
            (Some(input), _) | (_, Some(input)) => Some(input),
            _ => None,
        }?;
        [a, b]
            .into_iter()
            .find(|n| n.monitor.input == Some(observed))
            .map(|n| n.key.as_str())
    }

    pub fn input_devices<'a>(
        &self,
        here: &'a Node,
        other: &'a Node,
        synced: bool,
    ) -> InputDevices<'a> {
        if !synced {
            return InputDevices::default();
        }
        let (source, target) = if self.input_source.as_ref() == Some(&here.key) {
            (here, other)
        } else {
            (other, here)
        };
        let forwarded = self.input_source.as_ref() == Some(&source.key)
            && source.hardware.input()
            && source.input == Some(InputStatus::Peer)
            && target.input == Some(InputStatus::Here)
            && source.input_error.is_none()
            && target.input_error.is_none();
        if forwarded {
            return if here.key == target.key {
                InputDevices {
                    keyboard: if self.keyboard_enabled {
                        Some(source.key.as_str())
                    } else {
                        here.hardware.keyboard.then_some(here.key.as_str())
                    },
                    mouse: if self.mouse_enabled {
                        Some(source.key.as_str())
                    } else {
                        here.hardware.mouse.then_some(here.key.as_str())
                    },
                }
            } else {
                InputDevices {
                    keyboard: (!self.keyboard_enabled && here.hardware.keyboard)
                        .then_some(here.key.as_str()),
                    mouse: (!self.mouse_enabled && here.hardware.mouse)
                        .then_some(here.key.as_str()),
                }
            };
        }
        // Presence is independent of capture permissions and connection readiness.
        InputDevices {
            keyboard: here.hardware.keyboard.then_some(here.key.as_str()),
            mouse: here.hardware.mouse.then_some(here.key.as_str()),
        }
    }

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
        .filter(|_| {
            shared.keyboard_enabled
                || shared.mouse_enabled
                || shared.monitor_enabled
                    && Self::shared_monitor(a, b)
                    && a.monitor
                        .input
                        .zip(b.monitor.input)
                        .is_some_and(|(a, b)| a != 0 && b != 0 && a != b)
        })
        .filter(|source| {
            let target = if source.key == a.key { b } else { a };
            (shared.keyboard_enabled || target.hardware.keyboard)
                && (shared.mouse_enabled || target.hardware.mouse)
        })
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
            (shared.monitor_enabled
                && host.enabled
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
            keyboard_enabled: shared.keyboard_enabled,
            mouse_enabled: shared.mouse_enabled,
            audio_output,
            monitor,
            edge: shared.edge && shared.mouse_enabled,
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
            monitor_input: None,
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
    fn input_presence_survives_pending_or_failed_sharing_and_tracks_hotplug() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        let plan = Plan::build(&a, &b);
        let own = InputDevices {
            keyboard: Some("a"),
            mouse: Some("a"),
        };
        assert_eq!(plan.input_devices(&a, &b, false), InputDevices::default());
        assert_eq!(plan.input_devices(&a, &b, true), own);
        a.input_error = Some("permission denied".into());
        assert_eq!(plan.input_devices(&a, &b, true), own);
        a.input_error = None;
        a.input = Some(InputStatus::Peer);
        assert_eq!(plan.input_devices(&a, &b, true), own);
        b.input = Some(InputStatus::Here);
        assert_eq!(plan.input_devices(&a, &b, true), InputDevices::default());
        assert_eq!(plan.input_devices(&b, &a, true), own);
        b.input = None;
        assert_eq!(plan.input_devices(&a, &b, true), own);
        assert_eq!(plan.input_devices(&b, &a, true), InputDevices::default());
        a.hardware.keyboard = false;
        a.hardware.mouse = false;
        b.hardware.keyboard = true;
        b.hardware.mouse = true;
        // Old transport status cannot create phantom devices after unplugging.
        assert_eq!(plan.input_devices(&a, &b, true), InputDevices::default());
        let plan = Plan::build(&b, &a);
        assert_eq!(plan.input_source.as_deref(), Some("b"));
        assert_eq!(
            plan.input_devices(&b, &a, true),
            InputDevices {
                keyboard: Some("b"),
                mouse: Some("b")
            }
        );
        a.hardware.keyboard = true;
        let plan = Plan::build(&a, &b);
        assert_eq!(
            plan.input_devices(&a, &b, true),
            InputDevices {
                keyboard: Some("a"),
                mouse: None
            }
        );
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

    #[test]
    fn shared_monitor_colour_uses_confirmed_video_input_independently_of_input_handoff() {
        let mut a = node("a");
        let mut b = node("b");
        for (node, input) in [(&mut a, 17), (&mut b, 15)] {
            node.monitor.enabled = true;
            node.monitor.fingerprint = "shared display".into();
            node.monitor.input = Some(input);
        }
        a.monitor_input = Some(17);
        assert!(Plan::shared_monitor(&a, &b));
        assert_eq!(Plan::monitor_owner(&a, &b, false), None);
        assert_eq!(Plan::monitor_owner(&a, &b, true), Some("a"));
        a.input = Some(InputStatus::Peer);
        b.input = Some(InputStatus::Here);
        assert_eq!(Plan::monitor_owner(&a, &b, true), Some("a"));
        b.monitor_input = Some(15);
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        a.monitor_input = None;
        assert_eq!(Plan::monitor_owner(&a, &b, true), Some("b"));
        assert_eq!(Plan::monitor_owner(&b, &a, true), Some("b"));
        b.monitor_input = Some(99);
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        b.monitor_input = None;
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        a.monitor_input = Some(17);
        b.monitor.input = None;
        assert_eq!(Plan::monitor_owner(&a, &b, true), None);
        b.monitor.fingerprint = "another display".into();
        assert!(!Plan::shared_monitor(&a, &b));
    }

    #[test]
    fn source_audio_levels_survive_speaker_changes_and_sync_from_either_pc() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.audio_score = 40;
        b.hardware.audio_score = 20;
        let mut shared = Shared::default();
        assert!(shared.prepare_audio_levels("a", "b", Some("a")));
        assert_eq!(shared.audio_level("a").volume, 100);
        assert_eq!(shared.audio_level("b").volume, 50);
        shared.audio_levels.get_mut("a").unwrap().volume = 90;
        shared.audio_levels.get_mut("b").unwrap().muted = true;
        shared.changed("a");
        b.shared.merge(&shared);
        a.shared = shared.clone();
        let plan = Plan::build(&a, &b);
        b.shared.audio_levels.get_mut("b").unwrap().volume = 25;
        b.shared.changed("b");
        a.shared.merge(&b.shared);
        assert_eq!(plan.digest(&a, &b), Plan::build(&a, &b).digest(&a, &b));
        a.shared.audio = Selection::Computer("b".into());
        a.shared.changed("a");
        b.shared.merge(&a.shared);
        assert!(!a.shared.prepare_audio_levels("a", "b", Some("b")));
        assert_eq!(a.shared.audio_levels, b.shared.audio_levels);
        assert_eq!(a.shared.audio_level("a").volume, 90);
        assert_eq!(
            a.shared.audio_level("b"),
            AudioLevel {
                volume: 25,
                muted: true
            }
        );
        assert_eq!(Plan::build(&a, &b).audio_output.as_deref(), Some("b"));
    }

    #[test]
    fn sharing_cannot_remove_the_receivers_only_keyboard_or_mouse() {
        let mut a = node("a");
        let mut b = node("b");
        a.hardware.keyboard = true;
        a.hardware.mouse = true;
        a.input = Some(InputStatus::Peer);
        b.input = Some(InputStatus::Here);
        assert!(!Plan::can_disable(&a, &b, true));
        assert!(!Plan::can_disable(&b, &a, false));
        b.input = None;
        assert!(!Plan::can_disable(&a, &b, true));
        a.input = Some(InputStatus::Here);
        b.input = Some(InputStatus::Here);
        assert!(!Plan::can_disable(&a, &b, true));
        a.input = Some(InputStatus::Peer);
        a.shared.keyboard_enabled = false;
        a.shared.changed("a");
        assert!(Plan::build(&a, &b).input_source.is_none());
        b.hardware.keyboard = true;
        assert!(Plan::can_disable(&a, &b, true));
        assert!(!Plan::can_disable(&a, &b, false));
        let plan = Plan::build(&a, &b);
        assert_eq!(plan.input_source.as_deref(), Some("a"));
        assert_eq!(plan.input_devices(&a, &b, true).keyboard, Some("a"));
        assert_eq!(plan.input_devices(&b, &a, true).keyboard, Some("b"));
        assert_eq!(plan.input_devices(&b, &a, true).mouse, Some("a"));
        a.input = Some(InputStatus::Here);
        b.input = Some(InputStatus::Peer);
        assert!(Plan::can_disable(&a, &b, true));
        assert!(Plan::can_disable(&a, &b, false));
        a.shared.mouse_enabled = false;
        a.shared.monitor_enabled = false;
        a.shared.changed("a");
        assert!(Plan::build(&a, &b).input_source.is_none());
        assert!(a.shared.audio_enabled);
    }
}
